//! Painting the flyout: turn the display [`Model`] + a laid-out screen into pixels.
//!
//! [`render_page`] paints the panel's static layer; [`compose`] copies it and overlays what
//! changes without a relayout (slider fill and glow, hover).

use ab_glyph::FontVec;

use crate::icons;

use crate::canvas::{fit_label, lerp3, measure, Canvas, Rect};
use super::layout::{footer_btn_center_x, footer_buttons, footer_item_right, ActionKind, Elem, LaidElem};
use super::model::{AppRow, Group, Model, NowPlaying, Voice};
use super::theme::*;
use super::Interaction;
use crate::lang::tr;

/// The render context shared by both passes: model, accent, DPI `scale` and panel size.
pub(super) struct Ctx<'a> {
    pub model: &'a Model,
    pub accent: [u8; 3],
    pub scale: f32,
    pub width: i32,
    pub height: i32,
}

impl<'a> Ctx<'a> {
    pub(super) fn new(model: &'a Model, accent: [u8; 3], scale: f32, size: (i32, i32)) -> Self {
        Ctx { model, accent, scale, width: size.0, height: size.1 }
    }

    /// DIPs to whole pixels.
    fn d(&self, v: f32) -> i32 {
        (v * self.scale).round() as i32
    }
}

/// What the static layer's rows share: the fonts and the sizes every row reads.
struct Page<'a> {
    ctx: &'a Ctx<'a>,
    font: Option<&'static FontVec>,
    /// Semibold, else the regular font.
    font_sb: Option<&'static FontVec>,
    text_px: f32,
    hdr_px: f32,
    icon_px: u32,
}

/// A glyph `size` pixels in colour `col`, blitted with its top-left at `(x, y)` from its size.
fn blit_glyph(cv: &mut Canvas, glyph: char, size: u32, col: [u8; 3], alpha: f32, at: impl FnOnce(u32, u32) -> (i32, i32)) {
    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, size, col) {
        let (x, y) = at(gw, gh);
        cv.blit(x, y, &rgba, gw, gh, alpha);
    }
}

/// Render the panel's static layer into `out`.
pub(super) fn render_page(ctx: &Ctx, elems: &[LaidElem], out: &mut [u8]) {
    let (w, h) = (ctx.width, ctx.height);
    let mut cv = Canvas::new(out, w, h);

    cv.clear();
    cv.fill_round_rect(Rect::new(0.0, 0.0, w as f32, h as f32), ctx.d(CORNER) as f32, tint(), tint_a());

    let font = ui_font();
    let page = Page {
        ctx,
        font,
        font_sb: ui_font_semibold().or(font),
        text_px: 14.0 * ctx.scale,
        hdr_px: 14.0 * ctx.scale,
        icon_px: (ICON_PX * ctx.scale).round() as u32,
    };

    for le in elems {
        match le.elem {
            Elem::AppsHeader => page.apps_header(&mut cv, le),
            Elem::Pager => page.pager(&mut cv, le),
            Elem::GroupHeader { group } => page.group_header(&mut cv, le, group),
            Elem::NowPlaying { index } => {
                if let Some(np) = ctx.model.now_playing.get(index) {
                    page.now_playing(&mut cv, le, np);
                }
            }
            Elem::Voice => {
                if let Some(voice) = &ctx.model.voice {
                    page.voice(&mut cv, le, voice);
                }
            }
            Elem::AppSlider { app } => page.app_row(&mut cv, le, &ctx.model.apps[app]),
            Elem::Note => page.note(&mut cv, le),
            Elem::Slider { group } => page.slider_row(&mut cv, le, &ctx.model.groups[group]),
            Elem::Device { group, dev } => page.device(&mut cv, le, group, dev),
            Elem::BatteriesHeader => {
                page.header_label(&mut cv, le, tr("Batteries", "Batteries"));
            }
            Elem::Battery { index } => page.battery(&mut cv, le, &ctx.model.batteries[index]),
            Elem::Footer => page.footer(&mut cv, le),
        }
    }
}

impl Page<'_> {
    /// A section header's label baseline in its row.
    fn header_base(&self, le: &LaidElem) -> f32 {
        le.top as f32 + le.height as f32 - self.hdr_px * 0.55
    }

    /// A section header's label, semibold; returns its baseline.
    fn header_label(&self, cv: &mut Canvas, le: &LaidElem, label: &str) -> f32 {
        let base = self.header_base(le);
        if let Some(f) = self.font_sb {
            cv.draw_text(f, self.hdr_px, (self.ctx.d(HEADER_X) as f32, base), text(), 1.0, label);
        }
        base
    }

    fn apps_header(&self, cv: &mut Canvas, le: &LaidElem) {
        let model = self.ctx.model;
        let base = self.header_label(cv, le, tr("Apps", "Applications"));
        // The count says what folding hides; a list cut short says which rows it shows.
        let all = model.apps.len();
        let shown = super::layout::shown_apps(model);
        let count = match all {
            _ if !model.fit.apps_folded && shown.len() < all => {
                let (first, last) = (shown.start + 1, shown.end);
                crate::tf!("{first}–{last} of {all}", "{first}–{last} sur {all}")
            }
            1 => tr("1 app", "1 application").to_string(),
            n => crate::tf!("{n} apps", "{n} applications"),
        };
        header_fold(cv, self.ctx.scale, self.ctx.width, base, self.hdr_px, !model.fit.apps_folded, &count, self.font);
    }

    fn pager(&self, cv: &mut Canvas, le: &LaidElem) {
        let (scale, w) = (self.ctx.scale, self.ctx.width);
        let n = self.ctx.model.now_playing.len();
        let cy = (le.top + le.height / 2) as f32;
        let r = PIP_D * scale / 2.0;
        for i in 0..n {
            let cx = super::layout::pip_center_x(w, scale, n, i);
            if i == self.ctx.model.page {
                let half = PIP_ON_W * scale / 2.0;
                cv.fill_round_rect(Rect::new(cx - half, cy - r, cx + half, cy + r), r, self.ctx.accent, 1.0);
            } else {
                cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, text(), 0.35);
            }
        }
    }

    fn group_header(&self, cv: &mut Canvas, le: &LaidElem, group: usize) {
        let g = &self.ctx.model.groups[group];
        let base = self.header_label(cv, le, g.title);
        // More than one device: a count and a chevron say the list folds.
        if g.devices.len() > 1 {
            let expanded = self.ctx.model.expanded.get(group).copied().unwrap_or(false);
            let n = g.devices.len();
            header_fold(cv, self.ctx.scale, self.ctx.width, base, self.hdr_px, expanded, &crate::tf!("{n} devices", "{n} appareils"), self.font);
        }
    }

    fn now_playing(&self, cv: &mut Canvas, le: &LaidElem, np: &NowPlaying) {
        let (scale, w) = (self.ctx.scale, self.ctx.width);
        let art = (super::model::ART_DIP * scale).round() as i32;
        let ax = self.ctx.d(ART_X);
        let ay = le.top + (le.height - art) / 2;
        match &np.art {
            Some(sprite) => cv.blit(ax, ay, sprite, art as u32, art as u32, 1.0),
            None => {
                cv.fill_round_rect(Rect::new(ax as f32, ay as f32, (ax + art) as f32, (ay + art) as f32), art as f32 * 0.15, text(), 0.08);
                let glyph = if np.video { GLYPH_VIDEO } else { GLYPH_MUSIC };
                blit_glyph(cv, glyph, (art as f32 * 0.42) as u32, text(), 0.7, |gw, gh| (ax + (art - gw as i32) / 2, ay + (art - gh as i32) / 2));
            }
        }
        let tx = (ax + art) as f32 + 12.0 * scale;
        let max_w = super::layout::media_button_center_x(w, scale, 0) - MEDIA_BTN * scale / 2.0 - tx - 4.0 * scale;
        let cy = (le.top + le.height / 2) as f32;
        if let Some(f) = self.font_sb {
            let line = fit_label(f, self.text_px, &np.title, max_w);
            cv.draw_text(f, self.text_px, (tx, cy - 6.0 * scale), text(), 1.0, &line);
        }
        if let Some(f) = self.font {
            let px = 12.5 * scale;
            let live = if np.live { tr(" · LIVE", " · EN DIRECT") } else { "" };
            let line = fit_label(f, px, &format!("{} · {}{live}", np.artist, np.app), max_w);
            cv.draw_text(f, px, (tx, cy + 13.0 * scale), text(), 0.68, &line);
        }
        let bpx = (16.0 * scale).round() as u32;
        for (i, glyph) in [GLYPH_PREVIOUS, if np.playing { GLYPH_PAUSE } else { GLYPH_PLAY }, GLYPH_NEXT].into_iter().enumerate() {
            // Previous/next greyed out where they do not reach the player, as on the pill.
            let alpha = if !np.skip && i != 1 { 0.3 } else { 1.0 };
            blit_glyph(cv, glyph, bpx, text(), alpha, |gw, gh| {
                let bx = super::layout::media_button_center_x(w, scale, i) - gw as f32 / 2.0;
                (bx.round() as i32, cy as i32 - gh as i32 / 2)
            });
        }
    }

    fn voice(&self, cv: &mut Canvas, le: &LaidElem, voice: &Voice) {
        let (scale, w) = (self.ctx.scale, self.ctx.width);
        let cy = (le.top + le.height / 2) as f32;
        let x = self.faces(cv, cy, voice);
        let max_w = super::layout::voice_button_center_x(w, scale) - VOICE_BTN * scale / 2.0 - x - 6.0 * scale;
        if let Some(f) = self.font_sb {
            let line = fit_label(f, self.text_px, &voice.title, max_w);
            cv.draw_text(f, self.text_px, (x, cy - 4.0 * scale), text(), 1.0, &line);
        }
        if let Some(f) = self.font {
            let px = 12.5 * scale;
            let line = fit_label(f, px, &voice.call.status_text(voice.muted), max_w);
            cv.draw_text(f, px, (x, cy + 14.0 * scale), text(), 0.68, &line);
        }
        // The mute button: a disc, accent when muted.
        let bx = super::layout::voice_button_center_x(w, scale);
        let r = VOICE_BTN * scale / 2.0;
        let (col, a) = if voice.muted { (self.ctx.accent, 1.0) } else { (text(), 0.10) };
        cv.fill_round_rect(Rect::new(bx - r, cy - r, bx + r, cy + r), r, col, a);
        let glyph = if voice.muted { GLYPH_MIC_OFF } else { GLYPH_MIC };
        let gcol = if voice.muted { DARK_GLYPH } else { text() };
        blit_glyph(cv, glyph, (16.0 * scale) as u32, gcol, 1.0, |gw, gh| ((bx - gw as f32 / 2.0).round() as i32, cy as i32 - gh as i32 / 2));
    }

    /// The voice panel's overlapping faces, the speakers ringed green, or a mic glyph when nobody
    /// is listed. Returns where the text after them starts.
    fn faces(&self, cv: &mut Canvas, cy: f32, voice: &Voice) -> f32 {
        let scale = self.ctx.scale;
        let face = FACE_DIP * scale;
        let mut x = ICON_X * scale;
        let members = &voice.call.members;
        if members.is_empty() {
            blit_glyph(cv, GLYPH_MIC, self.icon_px, text(), 1.0, |_, gh| (x as i32, cy as i32 - gh as i32 / 2));
            return x + self.icon_px as f32 + 14.0 * scale;
        }
        for member in members.iter().take(5) {
            let r = face / 2.0;
            let cx = x + r;
            cv.fill_round_rect(Rect::new(cx - r - 2.0, cy - r - 2.0, cx + r + 2.0, cy + r + 2.0), r + 2.0, tint(), 1.0);
            cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, icons::name_colour(&member.name), 1.0);
            if let Some(f) = self.font_sb {
                let initial: String = member.name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                let px = 12.0 * scale;
                let tw = measure(f, px, &initial);
                cv.draw_text(f, px, (cx - tw / 2.0, cy + px * 0.36), text(), 1.0, &initial);
            }
            if member.speaking {
                ring(cv, cx, cy, r + 1.5, 2.0 * scale, SPEAKING);
            }
            x += face * 0.72;
        }
        x + face * 0.28 + 10.0 * scale
    }

    fn app_row(&self, cv: &mut Canvas, le: &LaidElem, row: &AppRow) {
        let (scale, w, accent) = (self.ctx.scale, self.ctx.width, self.ctx.accent);
        let d = |v: f32| self.ctx.d(v);
        let cy_i = le.top + le.height / 2;
        let col = if row.muted { accent } else { text() };
        let glyph = if row.muted { GLYPH_APP_MUTED } else { GLYPH_APP };
        let gpx = (16.0 * scale).round() as u32;
        match &row.icon {
            // The app's own icon, centred where the glyph would be; muted, faded under a
            // small mute badge (still the spot that toggles it).
            Some(sprite) => {
                let px = (APP_ICON_DIP * scale).round() as i32;
                let (ix, iy) = (d(ICON_X) + 2 + d(8.0) - px / 2, cy_i - px / 2);
                cv.blit(ix, iy, sprite, px as u32, px as u32, if row.muted { 0.35 } else { 1.0 });
                if row.muted {
                    blit_glyph(cv, GLYPH_APP_MUTED, (11.0 * scale).round() as u32, accent, 1.0, |gw, gh| (ix + px - gw as i32 / 2 - d(2.0), iy + px - gh as i32 / 2 - d(2.0)));
                }
            }
            None => blit_glyph(cv, glyph, gpx, col, 1.0, |_, gh| (d(ICON_X) + 2, cy_i - gh as i32 / 2)),
        }
        if let Some(f) = self.font {
            let px = 13.0 * scale;
            let max_w = (APP_TRACK_X0 - 14.0) * scale - d(TEXT_X - 8.0) as f32;
            let label = fit_label(f, px, &row.app, max_w);
            cv.draw_text(f, px, (d(TEXT_X - 8.0) as f32, cy_i as f32 + px * 0.34), text(), 1.0, &label);
        }
        static_track(cv, APP_TRACK_X0 * scale, w as f32 - VALUE_W * scale, cy_i as f32, scale);
    }

    fn note(&self, cv: &mut Canvas, le: &LaidElem) {
        if let (Some(f), Some(notice)) = (self.font, self.ctx.model.notice) {
            let scale = self.ctx.scale;
            let x = self.ctx.d(HEADER_X) as f32;
            let px = NOTE_TEXT_PX * scale;
            let base = (le.top + le.height / 2) as f32 + px * 0.34;
            let max_w = self.ctx.width as f32 - x - RIGHT_PAD * scale;
            let line = fit_label(f, px, notice, max_w);
            cv.draw_text(f, px, (x, base), text(), NOTE_A, &line);
        }
    }

    fn slider_row(&self, cv: &mut Canvas, le: &LaidElem, g: &Group) {
        let scale = self.ctx.scale;
        let cy_i = le.top + le.height / 2;
        // Muted draws in the accent (the OSD uses a warm tint instead).
        let col = if g.muted { self.ctx.accent } else { text() };
        let glyph = endpoint_glyph(g.flow, g.muted);
        if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, self.icon_px, col) {
            let (gx, gy) = (self.ctx.d(ICON_X), cy_i - gh as i32 / 2);
            cv.blit(gx, gy, &rgba, gw, gh, 1.0);
            // An app is holding the microphone open.
            if g.recording {
                recording_dot(cv, gx, gy, self.icon_px);
            }
        }
        // Track background; the accent fill + thumb + value are drawn in compose.
        static_track(cv, TRACK_X0 * scale, self.ctx.width as f32 - VALUE_W * scale, cy_i as f32, scale);
    }

    fn device(&self, cv: &mut Canvas, le: &LaidElem, group: usize, dev: usize) {
        let (scale, w) = (self.ctx.scale, self.ctx.width);
        let d = |v: f32| self.ctx.d(v);
        let mx = d(ROW_MARGIN) as f32;
        let row = &self.ctx.model.groups[group].devices[dev];
        let ry0 = le.top as f32 + 1.0;
        let ry1 = (le.top + le.height) as f32 - 1.0;
        if row.selected {
            cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, text(), SEL_A);
            let ph = d(PILL_H) as f32;
            let pw = d(PILL_W) as f32;
            let py0 = (ry0 + ry1) / 2.0 - ph / 2.0;
            let px0 = mx + d(2.0) as f32;
            cv.fill_round_rect(Rect::new(px0, py0, px0 + pw, py0 + ph), pw / 2.0, self.ctx.accent, 1.0);
        }
        let cy = le.top + le.height / 2;
        if let Ok((rgba, gw, gh)) = row.icon.render(self.icon_px, text()) {
            cv.blit(d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
        }
        // Truncate so the name never runs under the battery.
        let reserve = if row.battery.is_some() { BATTERY_W } else { RIGHT_PAD };
        self.row_label(cv, cy, &row.label, reserve);
        if let Some(pct) = row.battery {
            draw_battery(cv, scale, w as f32 - RIGHT_PAD * scale, cy as f32, pct, self.font);
        }
    }

    /// A row's label at [`TEXT_X`], cut short of `reserve` DIPs on the right.
    fn row_label(&self, cv: &mut Canvas, cy: i32, label: &str, reserve: f32) {
        if let Some(f) = self.font {
            let x = self.ctx.d(TEXT_X) as f32;
            let base = cy as f32 + self.text_px * 0.34;
            let max_w = self.ctx.width as f32 - x - reserve * self.ctx.scale;
            let label = fit_label(f, self.text_px, label, max_w);
            cv.draw_text(f, self.text_px, (x, base), text(), 1.0, &label);
        }
    }

    fn battery(&self, cv: &mut Canvas, le: &LaidElem, b: &crate::hub::Battery) {
        let scale = self.ctx.scale;
        let cy = le.top + le.height / 2;
        let x = self.ctx.d(ICON_X);
        blit_glyph(cv, b.kind.glyph(), self.icon_px, text(), 1.0, |_, gh| (x, cy - gh as i32 / 2));
        self.row_label(cv, cy, &b.name, BATTERY_W);
        draw_battery(cv, scale, self.ctx.width as f32 - RIGHT_PAD * scale, cy as f32, b.level, self.font);
    }

    fn footer(&self, cv: &mut Canvas, le: &LaidElem) {
        let (scale, w, accent) = (self.ctx.scale, self.ctx.width, self.ctx.accent);
        // A darker strip under a hairline; it reaches the bottom edge, so its fill follows the corners.
        let top = le.top as f32;
        let bottom = (le.top + le.height) as f32;
        cv.fill_round_rect_bottom(Rect::new(0.0, top, w as f32, bottom), self.ctx.d(CORNER) as f32, FOOTER_SHADE, footer_shade_a());
        let line = scale.round().max(1.0);
        cv.fill_round_rect(Rect::new(0.0, top, w as f32, top + line), 0.0, text(), DIVIDER_A);
        let cy = le.top + le.height / 2;
        let fpx = (FOOTER_ICON_PX * scale).round() as u32;
        // Left: the labelled item (glyph + text), like the battery readout.
        blit_glyph(cv, ActionKind::LEFT.glyph(), fpx, text(), 1.0, |gw, gh| {
            (((FOOTER_ICON_X + FOOTER_ICON_PX / 2.0) * scale).round() as i32 - gw as i32 / 2, cy - gh as i32 / 2)
        });
        if let Some(f) = self.font {
            let lpx = FOOTER_TEXT_PX * scale;
            let base = cy as f32 + lpx * 0.34;
            // Unrounded FOOTER_TEXT_X, as `footer_item_right` measures it, or the label may ellipsise.
            // Stop short of the *leftmost* button, whichever that is.
            let last = footer_buttons(self.ctx.model).len() - 1;
            let maxw = footer_btn_center_x(w, scale, last) - FOOTER_BTN * scale / 2.0 - (FOOTER_TEXT_X + FOOTER_GAP) * scale;
            let label = fit_label(f, lpx, ActionKind::LEFT.label(), maxw);
            cv.draw_text(f, lpx, (self.ctx.d(FOOTER_TEXT_X) as f32, base), text(), 1.0, &label);
        }
        // Right: the icon buttons, rightmost first; a call-to-action keeps an accent disc.
        for (i, k) in footer_buttons(self.ctx.model).into_iter().enumerate() {
            let cxb = footer_btn_center_x(w, scale, i);
            let col = if k == ActionKind::Restart { accent } else { text() };
            if k == ActionKind::Restart {
                let r = FOOTER_BTN * scale / 2.0;
                let cyf = cy as f32;
                cv.fill_round_rect(Rect::new(cxb - r, cyf - r, cxb + r, cyf + r), r, accent, FOOTER_CTA_A);
            }
            blit_glyph(cv, k.glyph(), fpx, col, 1.0, |gw, gh| ((cxb - gw as f32 / 2.0).round() as i32, cy - gh as i32 / 2));
        }
    }
}

/// A slider's track from `x0` to `x1` on line `cy`, in the static layer: only in the None motion
/// style, the others drawing their whole slider in compose (see `crate::motion`).
fn static_track(cv: &mut Canvas, x0: f32, x1: f32, cy: f32, scale: f32) {
    if crate::config::motion().volume == crate::config::MotionStyle::None {
        let th = TRACK_H * scale;
        cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, text(), 0.28);
    }
}

/// A section header's hover pill: [`FOOTER_ITEM_H`] tall, centred on the label that
/// [`render_page`] sets at `bottom - 0.55 em` (its cap height is about 0.7 em).
fn header_hover(cv: &mut Canvas, le: &LaidElem, w: i32, scale: f32) {
    let hdr_px = 14.0 * scale;
    let cy = (le.top + le.height) as f32 - hdr_px * (0.55 + 0.35);
    let h = FOOTER_ITEM_H * scale / 2.0;
    let mx = (ROW_MARGIN * scale).round();
    cv.fill_round_rect(Rect::new(mx, cy - h, w as f32 - mx, cy + h), (ROW_RADIUS * scale).round(), text(), HOVER_A);
}

/// Copy the static `base` into `buf`, then draw the dynamic overlays.
pub(super) fn compose(ctx: &Ctx, hit: &Interaction, elems: &[LaidElem], base: &[u8], buf: &mut [u8]) {
    buf.copy_from_slice(base);
    let mut cv = Canvas::new(buf, ctx.width, ctx.height);
    for le in elems {
        match le.elem {
            Elem::AppSlider { app } => app_slider(&mut cv, ctx, le, &ctx.model.apps[app]),
            Elem::Slider { group } => slider(&mut cv, ctx, le, &ctx.model.groups[group]),
            _ => {}
        }
    }
    for (idx, le) in elems.iter().enumerate() {
        hover(&mut cv, ctx, hit, le, hit.hover == Some(idx));
    }
}

/// A slider's value, in percent, at the panel's right on line `cy`: dimmed while `muted`.
fn draw_value(cv: &mut Canvas, ctx: &Ctx, cy: f32, level: f32, muted: bool) {
    if let Some(f) = ui_font() {
        let vpx = 13.0 * ctx.scale;
        let s = (level * 100.0).round().to_string();
        let tw = measure(f, vpx, &s);
        let vx = ctx.width as f32 - RIGHT_PAD * ctx.scale - tw;
        cv.draw_text(f, vpx, (vx, cy + vpx * 0.34), text(), if muted { 0.5 } else { 1.0 }, &s);
    }
}

/// An app's slider: the same fill and thumb as an endpoint's, without the live glow.
fn app_slider(cv: &mut Canvas, ctx: &Ctx, le: &LaidElem, row: &AppRow) {
    let scale = ctx.scale;
    let cy = le.top as f32 + le.height as f32 / 2.0;
    let x0 = APP_TRACK_X0 * scale;
    let x1 = ctx.width as f32 - VALUE_W * scale;
    let level = row.level.clamp(0.0, 1.0);
    // In the moving styles, the same look at the compact size, at its level: the hub
    // refreshes these rows, so they jump rather than spring.
    let style = crate::config::motion().volume;
    if style != crate::config::MotionStyle::None {
        let track = crate::motion::Track { x0, x1, cy, scale };
        let still = crate::motion::SliderMotion::still(level, row.muted);
        crate::motion::draw(cv, style, track, &still, row.muted, 0.0, ctx.accent, true);
        draw_value(cv, ctx, cy, level, row.muted);
        return;
    }
    let fx = x0 + (x1 - x0) * level;
    let th = TRACK_H * scale;
    let tr = THUMB_R * scale * 0.85;
    let (col, a) = if row.muted { (text(), 0.34) } else { (ctx.accent, 1.0) };
    if fx > x0 {
        cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, col, a);
    }
    cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, col, if row.muted { 0.5 } else { 1.0 });
    draw_value(cv, ctx, cy, level, row.muted);
}

/// An endpoint's slider: its fill and thumb, glowing with the live peak.
fn slider(cv: &mut Canvas, ctx: &Ctx, le: &LaidElem, g: &Group) {
    let (scale, accent) = (ctx.scale, ctx.accent);
    let cy = le.top as f32 + le.height as f32 / 2.0;
    let x0 = TRACK_X0 * scale;
    let x1 = ctx.width as f32 - VALUE_W * scale;
    let style = crate::config::motion().volume;
    if style != crate::config::MotionStyle::None {
        let track = crate::motion::Track { x0, x1, cy, scale };
        crate::motion::draw(cv, style, track, &g.motion, g.muted, g.peak, accent, false);
        draw_value(cv, ctx, cy, g.motion.shown(), g.muted);
        return;
    }
    let level = g.level.clamp(0.0, 1.0);
    let fx = x0 + (x1 - x0) * level;
    let th = TRACK_H * scale;
    let tr = THUMB_R * scale;
    if g.muted {
        // Muted: a flat, dim fill with no activity glow.
        if fx > x0 {
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, text(), 0.34);
        }
        cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, text(), 0.5);
    } else {
        // Additive glow from the live peak (none at rest); `powf` lifts low levels.
        let p = g.peak.clamp(0.0, 1.0).powf(0.55);
        // Bloom halo, under the fill.
        if p > 0.01 && fx > x0 {
            let bloom = lerp3(accent, text(), 0.35);
            let bh = th * (1.5 + 5.0 * p);
            cv.fill_round_rect(Rect::new(x0, cy - bh / 2.0, fx, cy + bh / 2.0), bh / 2.0, bloom, 0.30 * p);
        }
        // Base fill: full accent, lightening toward white as it glows.
        let fill_col = lerp3(accent, text(), 0.5 * p);
        if fx > x0 {
            cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, fill_col, 1.0);
        }
        // Thumb: matching glow plus a soft pulsing halo.
        if p > 0.01 {
            let halo = lerp3(accent, text(), 0.4);
            let hr = tr * (1.4 + 1.3 * p);
            cv.fill_round_rect(Rect::new(fx - hr, cy - hr, fx + hr, cy + hr), hr, halo, 0.32 * p);
        }
        cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, fill_col, 1.0);
    }
    draw_value(cv, ctx, cy, level, g.muted);
}

/// Hover highlights and the buttons' hover discs on row `le` (`hovered`: the pointer is on it).
fn hover(cv: &mut Canvas, ctx: &Ctx, hit: &Interaction, le: &LaidElem, hovered: bool) {
    let (scale, w) = (ctx.scale, ctx.width);
    let groups = &ctx.model.groups;
    let mx = ctx.d(ROW_MARGIN) as f32;
    let cy = (le.top + le.height / 2) as f32;
    // A round button's hover disc at `cx`.
    let disc = |cv: &mut Canvas, cx: f32, diameter: f32, col: [u8; 3], a: f32| {
        let r = diameter * scale / 2.0;
        cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, col, a);
    };
    match le.elem {
        // Whole-row highlight (a selected row already carries one in base).
        Elem::Device { group, dev } if hovered && !groups[group].devices[dev].selected => {
            let ry0 = le.top as f32 + 1.0;
            let ry1 = (le.top + le.height) as f32 - 1.0;
            cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), ctx.d(ROW_RADIUS) as f32, text(), HOVER_A);
        }
        Elem::Pager => {
            if let Some(i) = hit.hover_pip.filter(|&i| hovered && i != ctx.model.page) {
                let n = ctx.model.now_playing.len();
                disc(cv, super::layout::pip_center_x(w, scale, n, i), PIP_D, text(), 0.5);
            }
        }
        // A header's pill is centred on its label, which sits low in the row (the top is the
        // gap from the section above): the row's own bounds left the label at the pill's foot.
        Elem::AppsHeader if hovered => header_hover(cv, le, w, scale),
        Elem::GroupHeader { group } if hovered && groups[group].devices.len() > 1 => header_hover(cv, le, w, scale),
        Elem::NowPlaying { .. } if hovered => {
            if let Some(i) = hit.hover_media {
                disc(cv, super::layout::media_button_center_x(w, scale, i), MEDIA_BTN, text(), 0.10);
            }
        }
        Elem::Voice if hovered && hit.hover_voice => disc(cv, super::layout::voice_button_center_x(w, scale), VOICE_BTN, text(), 0.08),
        // Only the hovered footer item lights up, never the whole strip.
        Elem::Footer if hovered => {
            if hit.hover_footer == Some(ActionKind::LEFT) {
                let h = FOOTER_ITEM_H * scale / 2.0;
                let x1 = footer_item_right(scale);
                cv.fill_round_rect(Rect::new(mx, cy - h, x1, cy + h), ctx.d(ROW_RADIUS) as f32, text(), HOVER_A);
            }
            for (i, k) in footer_buttons(ctx.model).into_iter().enumerate() {
                if hit.hover_footer != Some(k) {
                    continue;
                }
                // A call-to-action deepens its accent disc; the rest get a neutral one.
                let (col, a) = match k {
                    ActionKind::Restart => (ctx.accent, FOOTER_CTA_HOVER_A - FOOTER_CTA_A),
                    _ => (text(), 0.10),
                };
                disc(cv, footer_btn_center_x(w, scale, i), FOOTER_BTN, col, a);
            }
        }
        _ => {}
    }
}

/// A folding header's right-hand side: `count` in secondary text, then a chevron pointing the
/// way a click goes. `base` is the header label's baseline.
#[allow(clippy::too_many_arguments)]
fn header_fold(cv: &mut Canvas, scale: f32, w: i32, base: f32, hdr_px: f32, open: bool, count: &str, font: Option<&FontVec>) {
    let cpx = (CHEVRON_PX * scale).round() as u32;
    let right = w as f32 - RIGHT_PAD * scale;
    blit_glyph(cv, if open { GLYPH_UP } else { GLYPH_DOWN }, cpx, text(), 0.8, |gw, gh| {
        ((right - gw as f32).round() as i32, (base - hdr_px * 0.36 - gh as f32 / 2.0).round() as i32)
    });
    if let Some(f) = font {
        let px = 12.5 * scale;
        let tw = measure(f, px, count);
        cv.draw_text(f, px, (right - cpx as f32 - 8.0 * scale - tw, base), text(), 0.62, count);
    }
}

/// A ring of `stroke` px at radius `r`, as the difference of two discs' coverage.
fn ring(cv: &mut Canvas, cx: f32, cy: f32, r: f32, stroke: f32, col: [u8; 3]) {
    let outer = r + stroke / 2.0;
    let inner = r - stroke / 2.0;
    for y in (cy - outer).floor() as i32..(cy + outer).ceil() as i32 {
        for x in (cx - outer).floor() as i32..(cx + outer).ceil() as i32 {
            let dist = ((x as f32 + 0.5 - cx).powi(2) + (y as f32 + 0.5 - cy).powi(2)).sqrt();
            let coverage = (outer - dist + 0.5).clamp(0.0, 1.0) * (dist - inner + 0.5).clamp(0.0, 1.0);
            if coverage > 0.0 {
                cv.fill_round_rect(Rect::new(x as f32, y as f32, x as f32 + 1.0, y as f32 + 1.0), 0.0, col, coverage);
            }
        }
    }
}

/// Draw the battery readout (a level glyph + "NN%") ending at right edge `right` (px).
fn draw_battery(cv: &mut Canvas, scale: f32, right: f32, cy: f32, pct: u8, font: Option<&FontVec>) {
    const DIM: f32 = 0.9;
    let pct = pct.min(100);
    let label = format!("{pct}%");
    let px = 12.5 * scale;
    let tw = font.map(|f| measure(f, px, &label)).unwrap_or(0.0);
    if let Some(f) = font {
        cv.draw_text(f, px, (right - tw, cy + px * 0.34), text(), DIM, &label);
    }
    // Segoe Fluent battery levels: Battery0 (E850, empty) … Battery9 (E859); full is Battery10,
    // apart at E83F (E85A is BatteryCharging0).
    let level = ((pct as f32 / 10.0).round() as u32).min(10);
    let glyph = if level == 10 { '\u{E83F}' } else { char::from_u32(0xE850 + level).unwrap_or('\u{E850}') };
    let size = (20.0 * scale).round() as u32;
    let gap = 5.0 * scale;
    blit_glyph(cv, glyph, size, text(), DIM, |gw, gh| ((right - tw - gap - gw as f32).round() as i32, (cy - gh as f32 / 2.0).round() as i32));
}
