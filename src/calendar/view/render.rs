//! The panel's frames: the scene drawn off screen (only where it changed, when little did), and the
//! card's pixels sent to its window (not at all when they are the same).

use super::*;

impl View {
    /// Draw the scene off screen.
    pub(super) fn draw_frame(&self) {
        let size = (self.px(self.scene.w) as u32, self.px(self.scene.h) as u32);
        // Nothing when nothing changed, the card where it was.
        let mut last = self.last.borrow_mut();
        let card = self.scene.main;
        let same = last.as_ref().is_some_and(|(was_size, was_card, items)| *was_size == size && *was_card == card && *items == self.scene.items);
        if same {
            return;
        }
        // Only where something changed (a hover, a pressed button, a line typed), over the same
        // frame: the rest of the bitmap stands.
        let areas = last.as_ref().filter(|(was_size, was_card, _)| *was_size == size && *was_card == card).and_then(|(_, _, old)| changed_areas(old, &self.scene.items, card));
        let drawn = self.gfx.borrow_mut().frame_in(size, self.scale, areas.as_deref(), |pen, area| draw(pen, &self.scene, &self.palette, area));
        match drawn {
            Ok(_) => *last = Some((size, card, self.scene.items.clone())),
            Err(e) => {
                *last = None;
                eprintln!("calendar: could not draw ({e:#})");
            }
        }
    }

    /// Draw and put on screen.
    pub(super) fn render(&self) {
        self.draw_frame();
        self.present((0, 0), 255);
    }

    /// The card's part of the frame to its window, `offset` from its place.
    pub(super) fn present(&self, (dx, dy): (i32, i32), alpha: u8) {
        let gfx = self.gfx.borrow();
        let full_w = self.px(self.scene.w).max(0);
        let row = full_w as usize * 4;
        let full_h = (gfx.pixels.len() / row.max(1)) as i32;
        // A card's rectangle of the frame, in pixels, kept inside it.
        let cut = |card: R| -> (i32, i32, i32, i32) {
            let (x0, y0) = (self.px(card.x).clamp(0, full_w), self.px(card.y).clamp(0, full_h));
            (x0, y0, self.px(card.w).min(full_w - x0).max(0), self.px(card.h).min(full_h - y0).max(0))
        };
        let line = |x0: i32, y0: i32, w: i32, r: i32| -> &[u8] {
            let start = (y0 + r) as usize * row + x0 as usize * 4;
            &gfx.pixels[start..start + w as usize * 4]
        };
        let (x, y) = (self.origin.0 + dx, self.origin.1 + dy);
        let sent = &mut *self.shown.borrow_mut();
        let (x0, y0, w, h) = cut(self.scene.main);
        let span = w as usize * 4;
        let at = (x + x0, y + y0);
        // A card that looks as it did is not sent again (a slide changes nothing): compared with
        // what its window holds, a memory compare.
        let same = sent.canvas.size() == (w, h) && {
            let held = sent.canvas.pixels();
            (0..h).all(|r| held[r as usize * span..][..span] == *line(x0, y0, w, r))
        };
        if !same {
            sent.canvas.present(self.hwnd, (w, h), at, alpha, |dst| {
                for (r, out) in dst.chunks_exact_mut(span.max(1)).enumerate().take(h as usize) {
                    out.copy_from_slice(line(x0, y0, w, r as i32));
                }
            });
        } else if (sent.at, sent.alpha) != (at, alpha) {
            // Only moved or faded (a slide): the window keeps its pixels.
            crate::layered::Canvas::shift(self.hwnd, at.0, at.1, alpha);
        }
        sent.at = at;
        sent.alpha = alpha;
    }
}

pub(super) fn overlaps(a: R, b: R) -> bool {
    a.x < b.right() && b.x < a.right() && a.y < b.bottom() && b.y < a.bottom()
}

/// Where the drawing `new` differs from `old`: the places of what went and what came (an item
/// unchanged at another index is no change), merged where they touch. `None`: redraw it all (too
/// much changed for parts to pay, or a clip or an unclip moved, which reshapes what follows).
pub(super) fn changed_areas(old: &[(R, Draw)], new: &[(R, Draw)], card: R) -> Option<Vec<R>> {
    let clips = |items: &[(R, Draw)]| items.iter().filter(|(_, d)| matches!(d, Draw::Clip | Draw::Unclip)).cloned().collect::<Vec<_>>();
    if clips(old) != clips(new) {
        return None;
    }
    let mut used = vec![false; new.len()];
    let mut areas: Vec<R> = Vec::new();
    for (i, item) in old.iter().enumerate() {
        // The same index first (most items stay where they were), else anywhere not yet matched.
        let found = if new.get(i) == Some(item) && !used[i] { Some(i) } else { new.iter().enumerate().position(|(j, n)| !used[j] && n == item) };
        match found {
            Some(j) => used[j] = true,
            None => areas.push(item.0),
        }
    }
    areas.extend(new.iter().zip(&used).filter(|(_, u)| !**u).map(|((r, _), _)| *r));
    // Merged where they overlap or touch, a pixel's slack for antialiased edges.
    let grown: Vec<R> = areas.into_iter().map(|r| R::new(r.x - 1.0, r.y - 1.0, r.w + 2.0, r.h + 2.0)).collect();
    let mut merged: Vec<R> = Vec::new();
    for r in grown {
        let mut r = r;
        while let Some(at) = merged.iter().position(|m| overlaps(*m, r)) {
            let m = merged.remove(at);
            let (x0, y0) = (m.x.min(r.x), m.y.min(r.y));
            r = R::new(x0, y0, m.right().max(r.right()) - x0, m.bottom().max(r.bottom()) - y0);
        }
        merged.push(r);
    }
    let changed: f32 = merged.iter().map(|r| r.w * r.h).sum();
    (!merged.is_empty() && merged.len() <= 16 && changed < card.w * card.h * 0.5).then_some(merged)
}

/// Draw `scene`, or only what falls in `area` (the frame is clipped to it).
pub(super) fn draw(pen: &Pen, scene: &Scene, p: &Palette, area: Option<R>) {
    pen.clear(Color(0.0, 0.0, 0.0, 0.0));
    let ink = |ink: Ink| match ink {
        Ink::Text => p.text,
        Ink::Text2 => p.text2,
        Ink::Text3 => p.text3,
        Ink::Accent => p.accent,
        Ink::OnAccent => p.on_accent,
        Ink::OnAccent2 => p.on_accent.alpha(0.78),
        Ink::Rgba(rgb, a) => Color::rgb(rgb).alpha(a as f32 / 255.0),
    };
    let fill = |fill: Fill| match fill {
        Fill::Raised => p.raised,
        Fill::Hover => p.hover,
        Fill::Pressed => p.text.alpha(0.12),
        Fill::Accent => p.accent,
        Fill::AccentHover => p.accent.alpha(0.85),
        Fill::AccentPressed => p.accent.alpha(0.7),
        Fill::Shade => p.shade,
        Fill::Divider => p.divider,
        Fill::Mark => p.text.alpha(0.5),
        Fill::Tip => p.tip,
        Fill::OnAccentWash => p.on_accent.alpha(0.18),
        Fill::Rgb(rgb) => Color::rgb(rgb),
        Fill::Rgba(rgb, a) => Color::rgb(rgb).alpha(a as f32 / 255.0),
    };
    for (r, d) in &scene.items {
        if area.is_some_and(|a| !overlaps(*r, a)) && !matches!(d, Draw::Clip | Draw::Unclip) {
            continue;
        }
        match d {
            Draw::Card => pen.fill(*r, 0.0, p.bg),
            Draw::Rect { fill: f, radius } => pen.fill(*r, *radius, fill(*f)),
            Draw::Outline { ink: i, radius } => pen.stroke(*r, *radius, 1.0, ink(*i).alpha(0.6)),
            Draw::Text { text, font, ink: i, align } => pen.text(text, *font, *r, ink(*i), *align),
            Draw::Glyph { glyph, font, ink: i } => pen.glyph(*glyph, *font, *r, ink(*i)),
            Draw::Logo { key, bytes } => pen.image(key, bytes, *r),
            Draw::Avatar { key, bytes } => pen.avatar(key, bytes, *r),
            Draw::Picture { key, bytes } => pen.picture(key, bytes, *r),
            Draw::Wash { rgb, alpha, reach, radius } => pen.wash(*r, *radius, Color::rgb(*rgb).alpha(*alpha as f32 / 255.0), *reach),
            Draw::Ring { rgb, alpha, width } => pen.ring(*r, *width, Color::rgb(*rgb).alpha(*alpha as f32 / 255.0)),
            Draw::Field { editor, font, caret } => pen.field(editor, *font, *r, [p.text, p.accent, p.on_accent], caret.then_some(p.text)),
            Draw::Clip => pen.clip(*r),
            Draw::Unclip => pen.unclip(),
        }
    }
}
