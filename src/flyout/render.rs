//! Painting the flyout: turn the display [`Model`] + a laid-out screen into pixels.
//!
//! [`render_page`] paints the panel's static layer; [`compose`] copies it and overlays what
//! changes without a relayout (slider fill and glow, hover).

use ab_glyph::FontVec;

use crate::icons;

use crate::canvas::{fit_label, lerp3, measure, Canvas, Rect};
use super::layout::{footer_btn_center_x, footer_buttons, footer_item_right, ActionKind, Elem, LaidElem};
use super::model::Model;
use super::theme::*;
use super::Interaction;

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
}

/// Render the panel's static layer into `out`.
pub(super) fn render_page(ctx: &Ctx, elems: &[LaidElem], out: &mut [u8]) {
    let scale = ctx.scale;
    let accent = ctx.accent;
    let (w, h) = (ctx.width, ctx.height);
    let d = |v: f32| (v * scale).round() as i32;
    let groups = &ctx.model.groups;
    let mut cv = Canvas::new(out, w, h);

    cv.clear();
    cv.fill_round_rect(Rect::new(0.0, 0.0, w as f32, h as f32), d(CORNER) as f32, TINT, TINT_A);

    let font = ui_font();
    let font_sb = ui_font_semibold().or(font);
    let text_px = 14.0 * scale;
    let hdr_px = 14.0 * scale;
    let icon_px = (ICON_PX * scale).round() as u32;
    let mx = d(ROW_MARGIN) as f32;

    for le in elems {
        match le.elem {
            Elem::AppsHeader => {
                let model = ctx.model;
                let base = le.top as f32 + le.height as f32 - hdr_px * 0.55;
                if let Some(f) = font_sb {
                    cv.draw_text(f, hdr_px, (d(HEADER_X) as f32, base), TEXT, 1.0, "Apps");
                }
                // The count says what folding hides; a list cut short says which rows it shows.
                let all = model.apps.len();
                let shown = super::layout::shown_apps(model);
                let count = match all {
                    _ if !model.fit.apps_folded && shown.len() < all => format!("{}–{} of {all}", shown.start + 1, shown.end),
                    1 => "1 app".to_string(),
                    n => format!("{n} apps"),
                };
                header_fold(&mut cv, scale, w, base, hdr_px, !model.fit.apps_folded, &count, font);
            }
            Elem::Pager => {
                let n = ctx.model.now_playing.len();
                let cy = (le.top + le.height / 2) as f32;
                let r = PIP_D * scale / 2.0;
                for i in 0..n {
                    let cx = super::layout::pip_center_x(w, scale, n, i);
                    if i == ctx.model.page {
                        let half = PIP_ON_W * scale / 2.0;
                        cv.fill_round_rect(Rect::new(cx - half, cy - r, cx + half, cy + r), r, accent, 1.0);
                    } else {
                        cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, TEXT, 0.35);
                    }
                }
            }
            Elem::GroupHeader { group } => {
                let g = &groups[group];
                let base = le.top as f32 + le.height as f32 - hdr_px * 0.55;
                if let Some(f) = font_sb {
                    cv.draw_text(f, hdr_px, (d(HEADER_X) as f32, base), TEXT, 1.0, g.title);
                }
                // More than one device: a count and a chevron say the list folds.
                if g.devices.len() > 1 {
                    let expanded = ctx.model.expanded.get(group).copied().unwrap_or(false);
                    header_fold(&mut cv, scale, w, base, hdr_px, expanded, &format!("{} devices", g.devices.len()), font);
                }
            }
            Elem::NowPlaying { index } => {
                if let Some(np) = ctx.model.now_playing.get(index) {
                    let art = (super::model::ART_DIP * scale).round() as i32;
                    let ax = d(ART_X);
                    let ay = le.top + (le.height - art) / 2;
                    match &np.art {
                        Some(sprite) => cv.blit(ax, ay, sprite, art as u32, art as u32, 1.0),
                        None => {
                            cv.fill_round_rect(Rect::new(ax as f32, ay as f32, (ax + art) as f32, (ay + art) as f32), art as f32 * 0.15, TEXT, 0.08);
                            if let Ok((rgba, gw, gh)) = icons::render_glyph(if np.video { GLYPH_VIDEO } else { GLYPH_MUSIC }, (art as f32 * 0.42) as u32, TEXT) {
                                cv.blit(ax + (art - gw as i32) / 2, ay + (art - gh as i32) / 2, &rgba, gw, gh, 0.7);
                            }
                        }
                    }
                    let tx = (ax + art) as f32 + 12.0 * scale;
                    let max_w = super::layout::media_button_center_x(w, scale, 0) - MEDIA_BTN * scale / 2.0 - tx - 4.0 * scale;
                    let cy = (le.top + le.height / 2) as f32;
                    if let Some(f) = font_sb {
                        let line = fit_label(f, text_px, &np.title, max_w);
                        cv.draw_text(f, text_px, (tx, cy - 6.0 * scale), TEXT, 1.0, &line);
                    }
                    if let Some(f) = font {
                        let px = 12.5 * scale;
                        let live = if np.live { " · LIVE" } else { "" };
                        let line = fit_label(f, px, &format!("{} · {}{live}", np.artist, np.app), max_w);
                        cv.draw_text(f, px, (tx, cy + 13.0 * scale), TEXT, 0.68, &line);
                    }
                    let bpx = (16.0 * scale).round() as u32;
                    for (i, glyph) in [GLYPH_PREVIOUS, if np.playing { GLYPH_PAUSE } else { GLYPH_PLAY }, GLYPH_NEXT].into_iter().enumerate() {
                        // Previous/next greyed out where they do not reach the player, as on the pill.
                        let alpha = if !np.skip && i != 1 { 0.3 } else { 1.0 };
                        if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, bpx, TEXT) {
                            let bx = super::layout::media_button_center_x(w, scale, i) - gw as f32 / 2.0;
                            cv.blit(bx.round() as i32, cy as i32 - gh as i32 / 2, &rgba, gw, gh, alpha);
                        }
                    }
                }
            }
            Elem::Voice => {
                if let Some(voice) = &ctx.model.voice {
                    let cy = (le.top + le.height / 2) as f32;
                    let face = FACE_DIP * scale;
                    let mut x = ICON_X * scale;
                    // Overlapping faces, the speakers ringed green; a mic glyph when nobody is listed.
                    if voice.members.is_empty() {
                        if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_MIC, icon_px, TEXT) {
                            cv.blit(x as i32, cy as i32 - gh as i32 / 2, &rgba, gw, gh, 1.0);
                        }
                        x += icon_px as f32 + 14.0 * scale;
                    } else {
                        for (name, speaking, _) in voice.members.iter().take(5) {
                            let r = face / 2.0;
                            let (cx, cyf) = (x + r, cy);
                            cv.fill_round_rect(Rect::new(cx - r - 2.0, cyf - r - 2.0, cx + r + 2.0, cyf + r + 2.0), r + 2.0, TINT, 1.0);
                            let hue = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
                            const PALETTE: [[u8; 3]; 6] = [[0x58, 0x65, 0xF2], [0x2D, 0x7D, 0x9A], [0x8E, 0x44, 0xAD], [0xC0, 0x39, 0x2B], [0x16, 0xA0, 0x85], [0xD3, 0x54, 0x00]];
                            cv.fill_round_rect(Rect::new(cx - r, cyf - r, cx + r, cyf + r), r, PALETTE[hue as usize % 6], 1.0);
                            if let Some(f) = font_sb {
                                let initial: String = name.chars().next().map(|c| c.to_uppercase().collect()).unwrap_or_default();
                                let px = 12.0 * scale;
                                let tw = measure(f, px, &initial);
                                cv.draw_text(f, px, (cx - tw / 2.0, cyf + px * 0.36), TEXT, 1.0, &initial);
                            }
                            if *speaking {
                                ring(&mut cv, cx, cyf, r + 1.5, 2.0 * scale, SPEAKING);
                            }
                            x += face * 0.72;
                        }
                        x += face * 0.28 + 10.0 * scale;
                    }
                    let max_w = super::layout::voice_button_center_x(w, scale) - VOICE_BTN * scale / 2.0 - x - 6.0 * scale;
                    if let Some(f) = font_sb {
                        let line = fit_label(f, text_px, &voice.title, max_w);
                        cv.draw_text(f, text_px, (x, cy - 4.0 * scale), TEXT, 1.0, &line);
                    }
                    if let Some(f) = font {
                        let px = 12.5 * scale;
                        let speaking: Vec<&str> = voice.members.iter().filter(|m| m.1).map(|m| m.0.as_str()).collect();
                        let status = if !speaking.is_empty() {
                            format!("{} talking", speaking.join(", "))
                        } else if voice.muted {
                            "You are muted".to_string()
                        } else if voice.members.is_empty() {
                            "Microphone live".to_string()
                        } else {
                            format!("{} in the call", voice.members.len())
                        };
                        let line = fit_label(f, px, &status, max_w);
                        cv.draw_text(f, px, (x, cy + 14.0 * scale), TEXT, 0.68, &line);
                    }
                    // The mute button: a disc, accent when muted.
                    let bx = super::layout::voice_button_center_x(w, scale);
                    let r = VOICE_BTN * scale / 2.0;
                    let (col, a) = if voice.muted { (accent, 1.0) } else { (TEXT, 0.10) };
                    cv.fill_round_rect(Rect::new(bx - r, cy - r, bx + r, cy + r), r, col, a);
                    let glyph = if voice.muted { GLYPH_MIC_OFF } else { GLYPH_MIC };
                    let gcol = if voice.muted { DARK_GLYPH } else { TEXT };
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, (16.0 * scale) as u32, gcol) {
                        cv.blit((bx - gw as f32 / 2.0).round() as i32, cy as i32 - gh as i32 / 2, &rgba, gw, gh, 1.0);
                    }
                }
            }
            Elem::AppSlider { app } => {
                let row = &ctx.model.apps[app];
                let cy_i = le.top + le.height / 2;
                let col = if row.muted { accent } else { TEXT };
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
                            if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_APP_MUTED, (11.0 * scale).round() as u32, accent) {
                                cv.blit(ix + px - gw as i32 / 2 - d(2.0), iy + px - gh as i32 / 2 - d(2.0), &rgba, gw, gh, 1.0);
                            }
                        }
                    }
                    None => {
                        if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, gpx, col) {
                            cv.blit(d(ICON_X) + 2, cy_i - gh as i32 / 2, &rgba, gw, gh, 1.0);
                        }
                    }
                }
                if let Some(f) = font {
                    let px = 13.0 * scale;
                    let max_w = (APP_TRACK_X0 - 14.0) * scale - d(TEXT_X - 8.0) as f32;
                    let label = fit_label(f, px, &row.app, max_w);
                    cv.draw_text(f, px, (d(TEXT_X - 8.0) as f32, cy_i as f32 + px * 0.34), TEXT, 1.0, &label);
                }
                // The other styles draw their whole slider in compose (see `crate::motion`).
                if crate::config::motion().volume == crate::config::MotionStyle::None {
                    let x0 = APP_TRACK_X0 * scale;
                    let x1 = w as f32 - VALUE_W * scale;
                    let th = TRACK_H * scale;
                    let cy = cy_i as f32;
                    cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.28);
                }
            }
            Elem::Note => {
                if let (Some(f), Some(text)) = (font, ctx.model.notice) {
                    let px = NOTE_TEXT_PX * scale;
                    let base = (le.top + le.height / 2) as f32 + px * 0.34;
                    let max_w = w as f32 - d(HEADER_X) as f32 - RIGHT_PAD * scale;
                    let line = fit_label(f, px, text, max_w);
                    cv.draw_text(f, px, (d(HEADER_X) as f32, base), TEXT, NOTE_A, &line);
                }
            }
            Elem::Slider { group } => {
                let g = &groups[group];
                let cy_i = le.top + le.height / 2;
                let cy = cy_i as f32;
                // Muted draws in the accent (the OSD uses a warm tint instead).
                let col = if g.muted { accent } else { TEXT };
                let glyph = endpoint_glyph(g.flow, g.muted);
                if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, icon_px, col) {
                    let (gx, gy) = (d(ICON_X), cy_i - gh as i32 / 2);
                    cv.blit(gx, gy, &rgba, gw, gh, 1.0);
                    // An app is holding the microphone open.
                    if g.recording {
                        recording_dot(&mut cv, gx, gy, icon_px);
                    }
                }
                // Track background; the accent fill + thumb + value are drawn in compose. The other
                // styles move their tracks too (see `crate::motion`): compose draws them whole.
                if crate::config::motion().volume == crate::config::MotionStyle::None {
                    let x0 = TRACK_X0 * scale;
                    let x1 = w as f32 - VALUE_W * scale;
                    let th = TRACK_H * scale;
                    cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.28);
                }
            }
            Elem::Device { group, dev } => {
                let row = &groups[group].devices[dev];
                let ry0 = le.top as f32 + 1.0;
                let ry1 = (le.top + le.height) as f32 - 1.0;
                if row.selected {
                    cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, TEXT, SEL_A);
                    let ph = d(PILL_H) as f32;
                    let pw = d(PILL_W) as f32;
                    let py0 = (ry0 + ry1) / 2.0 - ph / 2.0;
                    let px0 = mx + d(2.0) as f32;
                    cv.fill_round_rect(Rect::new(px0, py0, px0 + pw, py0 + ph), pw / 2.0, accent, 1.0);
                }
                let cy = le.top + le.height / 2;
                if let Ok((rgba, gw, gh)) = row.icon.render(icon_px, TEXT) {
                    cv.blit(d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                if let Some(f) = font {
                    let base = cy as f32 + text_px * 0.34;
                    // Truncate so the name never runs under the battery.
                    let reserve = if row.battery.is_some() { BATTERY_W } else { RIGHT_PAD };
                    let max_w = w as f32 - d(TEXT_X) as f32 - reserve * scale;
                    let label = fit_label(f, text_px, &row.label, max_w);
                    cv.draw_text(f, text_px, (d(TEXT_X) as f32, base), TEXT, 1.0, &label);
                }
                if let Some(pct) = row.battery {
                    draw_battery(&mut cv, scale, w as f32 - RIGHT_PAD * scale, cy as f32, pct, font);
                }
            }
            Elem::BatteriesHeader => {
                let base = le.top as f32 + le.height as f32 - hdr_px * 0.55;
                if let Some(f) = font_sb {
                    cv.draw_text(f, hdr_px, (d(HEADER_X) as f32, base), TEXT, 1.0, "Batteries");
                }
            }
            Elem::Battery { index } => {
                let b = &ctx.model.batteries[index];
                let cy = le.top + le.height / 2;
                if let Ok((rgba, gw, gh)) = icons::render_glyph(b.kind.glyph(), icon_px, TEXT) {
                    cv.blit(d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                if let Some(f) = font {
                    let base = cy as f32 + text_px * 0.34;
                    let max_w = w as f32 - d(TEXT_X) as f32 - BATTERY_W * scale;
                    let label = fit_label(f, text_px, &b.name, max_w);
                    cv.draw_text(f, text_px, (d(TEXT_X) as f32, base), TEXT, 1.0, &label);
                }
                draw_battery(&mut cv, scale, w as f32 - RIGHT_PAD * scale, cy as f32, b.level, font);
            }
            Elem::Footer => {
                // A darker strip under a hairline; it reaches the bottom edge, so its fill follows the corners.
                let top = le.top as f32;
                let bottom = (le.top + le.height) as f32;
                cv.fill_round_rect_bottom(
                    Rect::new(0.0, top, w as f32, bottom),
                    d(CORNER) as f32,
                    FOOTER_SHADE,
                    FOOTER_SHADE_A,
                );
                let line = scale.round().max(1.0);
                cv.fill_round_rect(Rect::new(0.0, top, w as f32, top + line), 0.0, TEXT, DIVIDER_A);
                let cy = le.top + le.height / 2;
                let fpx = (FOOTER_ICON_PX * scale).round() as u32;
                // Left: the labelled item (glyph + text), like the battery readout.
                if let Ok((rgba, gw, gh)) = icons::render_glyph(ActionKind::LEFT.glyph(), fpx, TEXT) {
                    let x = ((FOOTER_ICON_X + FOOTER_ICON_PX / 2.0) * scale).round() as i32 - gw as i32 / 2;
                    cv.blit(x, cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                if let Some(f) = font {
                    let lpx = FOOTER_TEXT_PX * scale;
                    let base = cy as f32 + lpx * 0.34;
                    // Unrounded FOOTER_TEXT_X, as `content_width` measured it, or the label may ellipsise.
                    // Stop short of the *leftmost* button, whichever that is.
                    let last = footer_buttons(ctx.model).len() - 1;
                    let maxw = footer_btn_center_x(w, scale, last) - FOOTER_BTN * scale / 2.0
                        - (FOOTER_TEXT_X + FOOTER_GAP) * scale;
                    let label = fit_label(f, lpx, ActionKind::LEFT.label(), maxw);
                    cv.draw_text(f, lpx, (d(FOOTER_TEXT_X) as f32, base), TEXT, 1.0, &label);
                }
                // Right: the icon buttons, rightmost first; a call-to-action keeps an accent disc.
                for (i, k) in footer_buttons(ctx.model).into_iter().enumerate() {
                    let cxb = footer_btn_center_x(w, scale, i);
                    let col = if k == ActionKind::Restart { accent } else { TEXT };
                    if k == ActionKind::Restart {
                        let r = FOOTER_BTN * scale / 2.0;
                        let cyf = cy as f32;
                        cv.fill_round_rect(Rect::new(cxb - r, cyf - r, cxb + r, cyf + r), r, accent, FOOTER_CTA_A);
                    }
                    if let Ok((rgba, gw, gh)) = icons::render_glyph(k.glyph(), fpx, col) {
                        let x = (cxb - gw as f32 / 2.0).round() as i32;
                        cv.blit(x, cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                    }
                }
            }
        }
    }
}

/// A section header's hover pill: [`FOOTER_ITEM_H`] tall, centred on the label that
/// [`render_page`] sets at `bottom - 0.55 em` (its cap height is about 0.7 em).
fn header_hover(cv: &mut Canvas, le: &LaidElem, w: i32, scale: f32) {
    let hdr_px = 14.0 * scale;
    let cy = (le.top + le.height) as f32 - hdr_px * (0.55 + 0.35);
    let h = FOOTER_ITEM_H * scale / 2.0;
    let mx = (ROW_MARGIN * scale).round();
    cv.fill_round_rect(Rect::new(mx, cy - h, w as f32 - mx, cy + h), (ROW_RADIUS * scale).round(), TEXT, HOVER_A);
}

/// Copy the static `base` into `buf`, then draw the dynamic overlays.
pub(super) fn compose(ctx: &Ctx, hit: &Interaction, elems: &[LaidElem], base: &[u8], buf: &mut [u8]) {
    buf.copy_from_slice(base);
    let scale = ctx.scale;
    let accent = ctx.accent;
    let (w, h) = (ctx.width, ctx.height);
    let d = |v: f32| (v * scale).round() as i32;
    let groups = &ctx.model.groups;
    let hover = hit.hover;
    let font = ui_font();
    let mx = d(ROW_MARGIN) as f32;
    let mut cv = Canvas::new(buf, w, h);

    for le in elems {
        // An app's slider: the same fill and thumb as an endpoint's, without the live glow.
        if let Elem::AppSlider { app } = le.elem {
            let row = &ctx.model.apps[app];
            let cy = le.top as f32 + le.height as f32 / 2.0;
            let x0 = APP_TRACK_X0 * scale;
            let x1 = w as f32 - VALUE_W * scale;
            let level = row.level.clamp(0.0, 1.0);
            // In the moving styles, the same look at the compact size, at its level: the hub
            // refreshes these rows, so they jump rather than spring.
            let style = crate::config::motion().volume;
            if style != crate::config::MotionStyle::None {
                let track = crate::motion::Track { x0, x1, cy, scale };
                let still = crate::motion::SliderMotion::still(level, row.muted);
                crate::motion::draw(&mut cv, style, track, &still, row.muted, 0.0, accent, true);
                if let Some(f) = font {
                    let vpx = 13.0 * scale;
                    let s = (level * 100.0).round().to_string();
                    let tw = measure(f, vpx, &s);
                    cv.draw_text(f, vpx, (w as f32 - RIGHT_PAD * scale - tw, cy + vpx * 0.34), TEXT, if row.muted { 0.5 } else { 1.0 }, &s);
                }
                continue;
            }
            let fx = x0 + (x1 - x0) * level;
            let th = TRACK_H * scale;
            let tr = THUMB_R * scale * 0.85;
            let (col, a) = if row.muted { (TEXT, 0.34) } else { (accent, 1.0) };
            if fx > x0 {
                cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, col, a);
            }
            cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, col, if row.muted { 0.5 } else { 1.0 });
            if let Some(f) = font {
                let vpx = 13.0 * scale;
                let s = (level * 100.0).round().to_string();
                let tw = measure(f, vpx, &s);
                cv.draw_text(f, vpx, (w as f32 - RIGHT_PAD * scale - tw, cy + vpx * 0.34), TEXT, if row.muted { 0.5 } else { 1.0 }, &s);
            }
        }
        if let Elem::Slider { group } = le.elem {
            let g = &groups[group];
            let cy = le.top as f32 + le.height as f32 / 2.0;
            let x0 = TRACK_X0 * scale;
            let x1 = w as f32 - VALUE_W * scale;
            let style = crate::config::motion().volume;
            if style != crate::config::MotionStyle::None {
                let track = crate::motion::Track { x0, x1, cy, scale };
                crate::motion::draw(&mut cv, style, track, &g.motion, g.muted, g.peak, accent, false);
                if let Some(f) = font {
                    let vpx = 13.0 * scale;
                    let s = (g.motion.shown() * 100.0).round().to_string();
                    let tw = measure(f, vpx, &s);
                    let vx = w as f32 - RIGHT_PAD * scale - tw;
                    cv.draw_text(f, vpx, (vx, cy + vpx * 0.34), TEXT, if g.muted { 0.5 } else { 1.0 }, &s);
                }
                continue;
            }
            let level = g.level.clamp(0.0, 1.0);
            let fx = x0 + (x1 - x0) * level;
            let th = TRACK_H * scale;
            let tr = THUMB_R * scale;
            if g.muted {
                // Muted: a flat, dim fill with no activity glow.
                if fx > x0 {
                    cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, TEXT, 0.34);
                }
                cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, TEXT, 0.5);
            } else {
                // Additive glow from the live peak (none at rest); `powf` lifts low levels.
                let p = g.peak.clamp(0.0, 1.0).powf(0.55);
                // Bloom halo, under the fill.
                if p > 0.01 && fx > x0 {
                    let bloom = lerp3(accent, TEXT, 0.35);
                    let bh = th * (1.5 + 5.0 * p);
                    cv.fill_round_rect(Rect::new(x0, cy - bh / 2.0, fx, cy + bh / 2.0), bh / 2.0, bloom, 0.30 * p);
                }
                // Base fill: full accent, lightening toward white as it glows.
                let fill_col = lerp3(accent, TEXT, 0.5 * p);
                if fx > x0 {
                    cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, fx, cy + th / 2.0), th / 2.0, fill_col, 1.0);
                }
                // Thumb: matching glow plus a soft pulsing halo.
                if p > 0.01 {
                    let halo = lerp3(accent, TEXT, 0.4);
                    let hr = tr * (1.4 + 1.3 * p);
                    cv.fill_round_rect(Rect::new(fx - hr, cy - hr, fx + hr, cy + hr), hr, halo, 0.32 * p);
                }
                cv.fill_round_rect(Rect::new(fx - tr, cy - tr, fx + tr, cy + tr), tr, fill_col, 1.0);
            }
            if let Some(f) = font {
                let vpx = 13.0 * scale;
                let s = (level * 100.0).round().to_string();
                let tw = measure(f, vpx, &s);
                let vx = w as f32 - RIGHT_PAD * scale - tw;
                let val_a = if g.muted { 0.5 } else { 1.0 };
                cv.draw_text(f, vpx, (vx, cy + vpx * 0.34), TEXT, val_a, &s);
            }
        }
    }

    // Hover highlights and the buttons' hover discs.
    for (idx, le) in elems.iter().enumerate() {
        let hovered = hover == Some(idx);
        match le.elem {
            // Whole-row highlight (a selected row already carries one in base).
            Elem::Device { group, dev } if hovered && !groups[group].devices[dev].selected => {
                let ry0 = le.top as f32 + 1.0;
                let ry1 = (le.top + le.height) as f32 - 1.0;
                cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, TEXT, HOVER_A);
            }
            Elem::Pager => {
                if let Some(i) = hit.hover_pip.filter(|&i| hovered && i != ctx.model.page) {
                    let n = ctx.model.now_playing.len();
                    let (cx, cy) = (super::layout::pip_center_x(w, scale, n, i), (le.top + le.height / 2) as f32);
                    let r = PIP_D * scale / 2.0;
                    cv.fill_round_rect(Rect::new(cx - r, cy - r, cx + r, cy + r), r, TEXT, 0.5);
                }
            }
            // A header's pill is centred on its label, which sits low in the row (the top is the
            // gap from the section above): the row's own bounds left the label at the pill's foot.
            Elem::AppsHeader if hovered => header_hover(&mut cv, le, w, scale),
            Elem::GroupHeader { group } if hovered && groups[group].devices.len() > 1 => header_hover(&mut cv, le, w, scale),
            Elem::NowPlaying { .. } if hovered => {
                if let Some(i) = hit.hover_media {
                    let cy = (le.top + le.height / 2) as f32;
                    let cxb = super::layout::media_button_center_x(w, scale, i);
                    let r = MEDIA_BTN * scale / 2.0;
                    cv.fill_round_rect(Rect::new(cxb - r, cy - r, cxb + r, cy + r), r, TEXT, 0.10);
                }
            }
            Elem::Voice if hovered && hit.hover_voice => {
                let cy = (le.top + le.height / 2) as f32;
                let cxb = super::layout::voice_button_center_x(w, scale);
                let r = VOICE_BTN * scale / 2.0;
                cv.fill_round_rect(Rect::new(cxb - r, cy - r, cxb + r, cy + r), r, TEXT, 0.08);
            }
            // Only the hovered footer item lights up, never the whole strip.
            Elem::Footer if hovered => {
                let cy = (le.top + le.height / 2) as f32;
                if hit.hover_footer == Some(ActionKind::LEFT) {
                    let h = FOOTER_ITEM_H * scale / 2.0;
                    let x1 = footer_item_right(scale);
                    cv.fill_round_rect(Rect::new(mx, cy - h, x1, cy + h), d(ROW_RADIUS) as f32, TEXT, HOVER_A);
                }
                for (i, k) in footer_buttons(ctx.model).into_iter().enumerate() {
                    if hit.hover_footer != Some(k) {
                        continue;
                    }
                    // A call-to-action deepens its accent disc; the rest get a neutral one.
                    let (col, a) = match k {
                        ActionKind::Restart => (accent, FOOTER_CTA_HOVER_A - FOOTER_CTA_A),
                        _ => (TEXT, 0.10),
                    };
                    let r = FOOTER_BTN * scale / 2.0;
                    let cxb = footer_btn_center_x(w, scale, i);
                    cv.fill_round_rect(Rect::new(cxb - r, cy - r, cxb + r, cy + r), r, col, a);
                }
            }
            _ => {}
        }
    }
}

/// A folding header's right-hand side: `count` in secondary text, then a chevron pointing the
/// way a click goes. `base` is the header label's baseline.
#[allow(clippy::too_many_arguments)]
fn header_fold(cv: &mut Canvas, scale: f32, w: i32, base: f32, hdr_px: f32, open: bool, count: &str, font: Option<&FontVec>) {
    let cpx = (CHEVRON_PX * scale).round() as u32;
    let right = w as f32 - RIGHT_PAD * scale;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(if open { GLYPH_UP } else { GLYPH_DOWN }, cpx, TEXT) {
        cv.blit((right - gw as f32).round() as i32, (base - hdr_px * 0.36 - gh as f32 / 2.0).round() as i32, &rgba, gw, gh, 0.8);
    }
    if let Some(f) = font {
        let px = 12.5 * scale;
        let tw = measure(f, px, count);
        cv.draw_text(f, px, (right - cpx as f32 - 8.0 * scale - tw, base), TEXT, 0.62, count);
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
    let text = format!("{pct}%");
    let px = 12.5 * scale;
    let tw = font.map(|f| measure(f, px, &text)).unwrap_or(0.0);
    if let Some(f) = font {
        cv.draw_text(f, px, (right - tw, cy + px * 0.34), TEXT, DIM, &text);
    }
    // Segoe Fluent battery levels: Battery0 (E850, empty) … Battery9 (E859); full is Battery10,
    // apart at E83F (E85A is BatteryCharging0).
    let level = ((pct as f32 / 10.0).round() as u32).min(10);
    let glyph = if level == 10 { '\u{E83F}' } else { char::from_u32(0xE850 + level).unwrap_or('\u{E850}') };
    let size = (20.0 * scale).round() as u32;
    let gap = 5.0 * scale;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, size, TEXT) {
        let x = (right - tw - gap - gw as f32).round() as i32;
        let y = (cy - gh as f32 / 2.0).round() as i32;
        cv.blit(x, y, &rgba, gw, gh, DIM);
    }
}
