//! Painting the flyout: turn the display [`Model`] + a laid-out screen into pixels.
//!
//! Two entry points, both pure over their inputs and both drawing through a [`Canvas`]:
//! [`render_page`] paints a screen's *static* layer (background, headers, device rows, the
//! slider track, the icon grid), and [`compose`] copies that base and overlays the *dynamic*
//! bits that change without a relayout — the slider fill/thumb/value with its activity glow,
//! the hover highlights, and the trailing battery/edit-pencil affordances.

use ab_glyph::FontVec;

use crate::audio::Flow;
use crate::icons::{self, IconId};

use super::canvas::{fit_label, lerp3, measure, Canvas, Rect};
use super::layout::{grid_metrics, pencil_center_x, Elem, LaidElem};
use super::model::Model;
use super::theme::*;
use super::Interaction;

/// The render context shared by both passes: what to draw ([`Model`]), the accent colour,
/// the DPI `scale`, and the panel size. Bundling these keeps the pass signatures small.
pub(super) struct Ctx<'a> {
    pub model: &'a Model,
    pub accent: [u8; 3],
    pub scale: f32,
    pub width: i32,
    pub height: i32,
}

/// Render `elems` (a screen) into `out` (a `width`×`height` RGBA buffer): the panel
/// background, then every element's static content. Pure — used both for the live base
/// layer and for the two frames composited during a slide transition.
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
            Elem::Header(text) => {
                if let Some(f) = font_sb {
                    let base = le.top as f32 + le.height as f32 - hdr_px * 0.55;
                    cv.draw_text(f, hdr_px, (d(HEADER_X) as f32, base), TEXT, 1.0, text);
                }
            }
            Elem::Slider { group } => {
                let g = &groups[group];
                let cy_i = le.top + le.height / 2;
                let cy = cy_i as f32;
                let (glyph, col) = match (g.flow, g.muted) {
                    (Flow::Output, false) => (GLYPH_VOLUME, TEXT),
                    (Flow::Output, true) => (GLYPH_MUTE, accent),
                    (Flow::Input, false) => (GLYPH_MIC, TEXT),
                    (Flow::Input, true) => (GLYPH_MIC_OFF, accent),
                };
                if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, icon_px, col) {
                    cv.blit(d(ICON_X), cy_i - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                // Track background; the accent fill + thumb + value are drawn in compose.
                let x0 = TRACK_X0 * scale;
                let x1 = w as f32 - VALUE_W * scale;
                let th = TRACK_H * scale;
                cv.fill_round_rect(Rect::new(x0, cy - th / 2.0, x1, cy + th / 2.0), th / 2.0, TEXT, 0.28);
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
                    // Leave the trailing zone free — truncate a long name so it never runs
                    // under the battery readout (or the hover pencil).
                    let reserve = if row.battery.is_some() { BATTERY_W } else { PENCIL_W };
                    let max_w = w as f32 - d(TEXT_X) as f32 - reserve * scale;
                    let label = fit_label(f, text_px, &row.label, max_w);
                    cv.draw_text(f, text_px, (d(TEXT_X) as f32, base), TEXT, 1.0, &label);
                }
                // The battery readout and the edit pencil both live on the right and are
                // mutually exclusive (pencil on hover, battery otherwise) — drawn in
                // `compose`, which knows the hover state.
            }
            Elem::PickerHeader { group, dev } => {
                let cy_i = le.top + le.height / 2;
                // The back chevron, centred in its button on the left.
                let bpx = (BACK_GLYPH_PX * scale).round() as u32;
                if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_BACK, bpx, TEXT) {
                    let bx = ((BACK_LEFT + BACK_BTN / 2.0) * scale).round() as i32 - gw as i32 / 2;
                    cv.blit(bx, cy_i - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                // The device name as the screen title.
                if let Some(f) = font_sb {
                    let title_px = TITLE_PX * scale;
                    let base = cy_i as f32 + title_px * 0.34;
                    let max_w = w as f32 - d(TEXT_X) as f32 - RIGHT_PAD * scale;
                    let title = fit_label(f, title_px, &groups[group].devices[dev].label, max_w);
                    cv.draw_text(f, title_px, (d(TEXT_X) as f32, base), TEXT, 1.0, &title);
                }
            }
            Elem::IconGrid { group, dev } => {
                let sel_icon = groups[group].devices[dev].icon;
                let (cols, left, chip, step) = grid_metrics(w, scale);
                let gy0 = le.top + (GRID_TOP_PAD * scale).round() as i32;
                let inner = (chip as f32 * GRID_ICON_RATIO).round() as u32;
                let r = d(8.0) as f32;
                for (idx, icon) in IconId::ALL.iter().enumerate() {
                    let cx0 = left + (idx as i32 % cols) * step;
                    let cy0 = gy0 + (idx as i32 / cols) * step;
                    let selected = *icon == sel_icon;
                    let (bg, a) = if selected { (accent, 1.0) } else { (TEXT, 0.06) };
                    cv.fill_round_rect(Rect::new(cx0 as f32, cy0 as f32, (cx0 + chip) as f32, (cy0 + chip) as f32), r, bg, a);
                    let gcol = if selected { DARK_GLYPH } else { TEXT };
                    if let Ok((rgba, gw, gh)) = icon.render(inner, gcol) {
                        let ox = cx0 + (chip - gw as i32) / 2;
                        let oy = cy0 + (chip - gh as i32) / 2;
                        cv.blit(ox, oy, &rgba, gw, gh, 1.0);
                    }
                }
            }
            Elem::Action(k) => {
                let cy = le.top + le.height / 2;
                if let Ok((rgba, gw, gh)) = icons::render_glyph(k.glyph(), icon_px, TEXT) {
                    cv.blit(d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                if let Some(f) = font {
                    let base = cy as f32 + text_px * 0.34;
                    cv.draw_text(f, text_px, (d(TEXT_X) as f32, base), TEXT, 1.0, k.label());
                }
            }
            Elem::UpdateBanner => {
                let cy = le.top + le.height / 2;
                // A subtle accent band marks it as a call-to-action.
                let ry0 = le.top as f32 + 1.0;
                let ry1 = (le.top + le.height) as f32 - 1.0;
                cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, accent, 0.16);
                if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_UPDATE, icon_px, accent) {
                    cv.blit(d(ICON_X), cy - gh as i32 / 2, &rgba, gw, gh, 1.0);
                }
                if let (Some(f), Some(label)) = (font, ctx.model.update_label()) {
                    let base = cy as f32 + text_px * 0.34;
                    let maxw = w as f32 - d(TEXT_X) as f32 - RIGHT_PAD * scale;
                    let label = fit_label(f, text_px, &label, maxw);
                    cv.draw_text(f, text_px, (d(TEXT_X) as f32, base), TEXT, 1.0, &label);
                }
            }
        }
    }
}

/// Copy the static `base`, then draw the dynamic overlays into `buf`: slider
/// fill/thumb/value (with the live activity glow), the hover highlight, and the
/// battery/edit-pencil affordances on the hovered device row.
pub(super) fn compose(ctx: &Ctx, hit: &Interaction, elems: &[LaidElem], base: &[u8], buf: &mut [u8]) {
    buf.copy_from_slice(base);
    let scale = ctx.scale;
    let accent = ctx.accent;
    let (w, h) = (ctx.width, ctx.height);
    let panel_w = ctx.width;
    let d = |v: f32| (v * scale).round() as i32;
    let groups = &ctx.model.groups;
    let hover = hit.hover;
    let hover_pencil = hit.hover_pencil;
    let hover_back = hit.hover_back;
    let hover_chip = hit.hover_chip;
    let font = ui_font();
    let mx = d(ROW_MARGIN) as f32;
    let mut cv = Canvas::new(buf, w, h);

    for le in elems {
        if let Elem::Slider { group } = le.elem {
            let g = &groups[group];
            let cy = le.top as f32 + le.height as f32 / 2.0;
            let x0 = TRACK_X0 * scale;
            let x1 = w as f32 - VALUE_W * scale;
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
                // The fill glows with the endpoint's live peak. The glow is purely
                // *additive*: at rest (p≈0) it's a normal full-accent slider — matching a
                // non-metered slider — and as audio rises it lightens toward white and grows
                // a soft bloom halo, with a pulsing halo around the thumb. `powf` lifts
                // low/mid levels (speech/music rarely peaks near 1.0) so it reads.
                let p = g.peak.clamp(0.0, 1.0).powf(0.55);
                // Outer bloom — the main "glow": a soft lightened-accent halo that grows tall
                // and more opaque with the level (drawn under the fill so it reads as a halo
                // above/below the track).
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

    // Right-hand affordances + hover highlights. On a device row the battery readout and the
    // edit pencil are mutually exclusive: pencil on the hovered row, battery otherwise (so
    // the current device still shows its battery when not hovered).
    for (idx, le) in elems.iter().enumerate() {
        let hovered = hover == Some(idx);
        match le.elem {
            Elem::Device { group, dev } => {
                let dev_row = &groups[group].devices[dev];
                let cy = (le.top + le.height / 2) as f32;
                if hovered {
                    // Whole-row highlight (selected row already carries one in base).
                    if !dev_row.selected {
                        let ry0 = le.top as f32 + 1.0;
                        let ry1 = (le.top + le.height) as f32 - 1.0;
                        cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, TEXT, HOVER_A);
                    }
                    // The battery stays visible but shifts left so the pencil can sit to its
                    // right (rather than replacing it).
                    if let Some(pct) = dev_row.battery {
                        let pencil_left = pencil_center_x(panel_w, scale) - PENCIL_BTN * scale / 2.0;
                        draw_battery(&mut cv, scale, pencil_left - 6.0 * scale, cy, pct, font);
                    }
                    // Round button behind the pencil, only when the pencil is hovered.
                    if hover_pencil {
                        let r = PENCIL_BTN * scale / 2.0;
                        let cxp = pencil_center_x(panel_w, scale);
                        cv.fill_round_rect(Rect::new(cxp - r, cy - r, cxp + r, cy + r), r, TEXT, 0.10);
                    }
                    let a = if hover_pencil { 1.0 } else { 0.85 };
                    draw_pencil(&mut cv, scale, panel_w, cy, a);
                } else if let Some(pct) = dev_row.battery {
                    draw_battery(&mut cv, scale, panel_w as f32 - RIGHT_PAD * scale, cy, pct, font);
                }
            }
            Elem::PickerHeader { .. } if hovered && hover_back => {
                // Round hover button behind the back arrow.
                let cy = (le.top + le.height / 2) as f32;
                let cxb = (BACK_LEFT + BACK_BTN / 2.0) * scale;
                let r = BACK_BTN * scale / 2.0;
                cv.fill_round_rect(Rect::new(cxb - r, cy - r, cxb + r, cy + r), r, TEXT, 0.10);
            }
            Elem::IconGrid { group, dev } => {
                // Brighten the chip under the cursor (accent chips lighten a touch too).
                if let Some(ci) = hover_chip.filter(|_| hovered) {
                    let (cols, left, chip, step) = grid_metrics(w, scale);
                    let gy0 = le.top + (GRID_TOP_PAD * scale).round() as i32;
                    let cx0 = left + (ci as i32 % cols) * step;
                    let cy0 = gy0 + (ci as i32 / cols) * step;
                    let r = d(8.0) as f32;
                    let selected = IconId::ALL[ci] == groups[group].devices[dev].icon;
                    let a = if selected { 0.14 } else { 0.10 };
                    cv.fill_round_rect(Rect::new(cx0 as f32, cy0 as f32, (cx0 + chip) as f32, (cy0 + chip) as f32), r, TEXT, a);
                }
            }
            Elem::Action(_) if hovered => {
                let ry0 = le.top as f32 + 1.0;
                let ry1 = (le.top + le.height) as f32 - 1.0;
                cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, TEXT, HOVER_A);
            }
            Elem::UpdateBanner if hovered => {
                // Deepen the accent band on hover.
                let ry0 = le.top as f32 + 1.0;
                let ry1 = (le.top + le.height) as f32 - 1.0;
                cv.fill_round_rect(Rect::new(mx, ry0, w as f32 - mx, ry1), d(ROW_RADIUS) as f32, accent, 0.14);
            }
            _ => {}
        }
    }
}

/// Draw the trailing "edit icon" pencil on a device row, centred on its button.
fn draw_pencil(cv: &mut Canvas, scale: f32, panel_w: i32, cy: f32, alpha: f32) {
    let size = (16.0 * scale).round() as u32;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(GLYPH_EDIT, size, TEXT) {
        let cx = pencil_center_x(panel_w, scale);
        let x = (cx - gw as f32 / 2.0).round() as i32;
        let y = (cy - gh as f32 / 2.0).round() as i32;
        cv.blit(x, y, &rgba, gw, gh, alpha);
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
    // Segoe Fluent battery levels: Battery0 (E850, empty) … Battery10 (E85A, full).
    let level = ((pct as f32 / 10.0).round() as u32).min(10);
    let glyph = char::from_u32(0xE850 + level).unwrap_or('\u{E850}');
    let size = (20.0 * scale).round() as u32;
    let gap = 5.0 * scale;
    if let Ok((rgba, gw, gh)) = icons::render_glyph(glyph, size, TEXT) {
        let x = (right - tw - gap - gw as f32).round() as i32;
        let y = (cy - gh as f32 / 2.0).round() as i32;
        cv.blit(x, y, &rgba, gw, gh, DIM);
    }
}
