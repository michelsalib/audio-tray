//! The calendar card laid out: the date and the world clocks, the month, the next event, the list;
//! or one event's details. Its height is summed from the same measures it is drawn with.

use super::*;

/// An event's details at most (shorter when the screen is).
pub(super) const DETAIL_H: f32 = 480.0;

/// The calendar card's parts, top to bottom, as [`calendar`] draws them and [`main_height`] sums
/// them: the date (and the gap under it), the month ([`grid_height`]), the next event and the gap
/// over it, the list's title and the gap over it, the rows; the gap over the footer.
pub(super) const DATE_H: f32 = 44.0;
pub(super) const DATE_GAP: f32 = 14.0;
pub(super) const HERO_GAP: f32 = 10.0;
pub(super) const HERO_H: f32 = 76.0;
pub(super) const LIST_GAP: f32 = 14.0;
pub(super) const LIST_TITLE_H: f32 = 24.0;
pub(super) const FOOT_GAP: f32 = 8.0;
/// The month's title row, its weekdays' row, its weeks, and the gap under it.
pub(super) const MONTH_TITLE_H: f32 = 32.0;
pub(super) const WEEKDAYS_H: f32 = 22.0;
pub(super) const WEEKS: f32 = 6.0;
pub(super) const GRID_GAP: f32 = 4.0;

/// The calendar card's height, the list's rows at most `rows`, with the next event or not.
pub(super) fn main_height(input: &Input, rows: usize, hero: bool) -> f32 {
    if input.detail.is_some() {
        return DETAIL_H;
    }
    let mut h = PAD + DATE_H + DATE_GAP;
    h += grid_height(input);
    if hero {
        h += HERO_GAP + HERO_H;
    }
    h += LIST_GAP + LIST_TITLE_H + rows.max(1) as f32 * ROW_H;
    if matches!(input.notice, Notice::Problem(_)) {
        h += PROBLEM_H;
    }
    h + FOOT_GAP + FOOTER_H
}

/// The calendar's height and what it shows: its list's rows (fewer on a short screen), and the
/// next event (left out when even one row would not fit with it).
pub(super) fn calendar_plan(input: &Input) -> (f32, usize, bool) {
    let room = input.room;
    let hero = input.hero.is_some() && main_height(input, 1, true) <= room;
    let mut rows = input.rows.len().clamp(1, input.rows_max.max(1));
    while rows > 1 && main_height(input, rows, hero) > room {
        rows -= 1;
    }
    let mut h = main_height(input, rows, hero);
    if input.detail.is_some() {
        // An event's details take what there is, never more than their height.
        h = h.min(room).max(240.0);
    }
    (h, rows, hero)
}

/// The calendar card. Returns the list's scrolling area.
pub(super) fn calendar(ink: &mut Layout, card: R, input: &Input, rows: usize, hero: bool) -> Option<(R, f32)> {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    let mut y = card.y + PAD;

    // The date and the world clocks.
    // Hovers here tint the text (a link's), not a plate: plates around short text read as boxes.
    let today = R::new(x, y, 200.0, 44.0);
    ink.hit(today, Hit::Today);
    let lit = ink.lit(Hit::Today);
    ink.text(R::new(x, y, 200.0, 16.0), input.weekday.clone(), Font::Caption, if lit { Ink::Accent } else { Ink::Text2 }, Align::Left);
    ink.text(R::new(x, y + 16.0, 220.0, 28.0), input.date.clone(), Font::Big, if lit { Ink::Accent } else { Ink::Text }, Align::Left);
    let edge = card.right() - PAD + 12.0;
    if !input.clocks.is_empty() {
        // On the weekday's line, at the card's edge: the big date beneath keeps its room (with
        // two clocks, they ran into it).
        let tw = (text_w(&input.clocks, Font::Caption) + 4.0).min(edge - 12.0 - x - 90.0).max(40.0);
        let r = R::new(edge - 12.0 - tw, y - 2.0, tw, 18.0);
        ink.hit(r, Hit::Clocks);
        let tint = if ink.lit(Hit::Clocks) { Ink::Accent } else { Ink::Text2 };
        ink.text(r, input.clocks.clone(), Font::Caption, tint, Align::Right);
    }
    y += DATE_H + DATE_GAP;

    // The month; nothing with the agenda alone.
    if input.grid != Grid::None {
        y = days_grid(ink, card, input, y);
    }

    // The next event.
    if let Some(hero) = input.hero.as_ref().filter(|_| hero) {
        y += HERO_GAP;
        let r = R::new(x, y, w, HERO_H);
        let fill = ink.surface(Hit::Hero, Fill::Raised);
        ink.draw(r, Draw::Rect { fill, radius: 6.0 });
        ink.hit(r, Hit::Hero);
        ink.draw(R::new(r.x + 12.0, r.y + 12.0, 3.0, r.h - 24.0), Draw::Rect { fill: Fill::Rgb(hero.color), radius: 1.5 });
        let tw = r.w - 40.0 - if hero.joinable { join_w() + 12.0 } else { 0.0 };
        ink.text(R::new(r.x + 26.0, r.y + 10.0, tw, 16.0), hero.caption.clone(), Font::TinyStrong, Ink::Accent, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 27.0, tw, 20.0), hero.title.clone(), Font::Strong, Ink::Text, Align::Left);
        ink.text(R::new(r.x + 26.0, r.y + 48.0, tw, 18.0), hero.when.clone(), Font::Caption, Ink::Text2, Align::Left);
        if hero.joinable {
            ink.join(r.right() - 12.0, r.y + 25.0, 26.0, Hit::HeroJoin);
        }
        y += HERO_H;
    }

    // The rest of the week, or the picked day.
    y += LIST_GAP;
    ink.text(R::new(x, y, w, 20.0), input.list_title.clone(), Font::CaptionStrong, Ink::Text2, Align::Left);
    y += LIST_TITLE_H;
    if let Notice::Problem(problem) = &input.notice {
        // A source failed: a line of its own, in the accent, a link to where it is mended (the
        // events under it may be old).
        let r = R::new(x, y, w, PROBLEM_H - 4.0);
        ink.hit(r, Hit::Settings);
        ink.glyph(R::new(r.x, r.y, 16.0, r.h), '\u{E7BA}', Font::IconSmall, Ink::Accent);
        let tint = if ink.lit(Hit::Settings) { Ink::Text } else { Ink::Accent };
        ink.text(R::new(r.x + 24.0, r.y, r.w - 24.0, r.h), format!("{problem} \u{2192}"), Font::Caption, tint, Align::Left);
        y += PROBLEM_H;
    }
    let area = R::new(card.x, y, card.w, rows.max(1) as f32 * ROW_H);
    let max = (input.rows.len() as f32 * ROW_H - area.h).max(0.0);
    let scroll = input.list_scroll.clamp(0.0, max);
    if input.rows.is_empty() {
        let line = R::new(x, y, w, ROW_H);
        match &input.notice {
            Notice::NoSources => {
                ink.hit(line, Hit::Settings);
                let tint = if ink.lit(Hit::Settings) { Ink::Accent } else { Ink::Text2 };
                ink.text(line, words().connect, Font::Body, tint, Align::Left);
            }
            Notice::Loading => ink.text(line, words().loading, Font::Body, Ink::Text3, Align::Left),
            _ => ink.text(line, input.empty.clone(), Font::Body, Ink::Text3, Align::Left),
        }
    }
    // The when column as wide as its longest label (12-hour times are longer), within reason.
    let left_w = input.rows.iter().map(|r| text_w(&r.left, Font::Caption) + 4.0).fold(76.0, f32::max).min(120.0);
    ink.draw(area, Draw::Clip);
    for (i, row) in input.rows.iter().enumerate() {
        let r = R::new(x - 6.0, area.y + i as f32 * ROW_H - scroll, w + 12.0, ROW_H);
        if r.bottom() <= area.y || r.y >= area.bottom() {
            continue;
        }
        ink.plate(r, Hit::Row(i), 4.0);
        ink.text(R::new(r.x + 6.0, r.y, left_w, r.h), row.left.clone(), Font::Caption, Ink::Text2, Align::Left);
        ink.draw(R::new(r.x + left_w + 12.0, r.y + 13.0, 6.0, 6.0), Draw::Rect { fill: Fill::Rgb(row.color), radius: 3.0 });
        let right_w = if row.joinable { join_w() + 8.0 } else if row.right.is_empty() { 0.0 } else { 56.0 };
        let tx = r.x + left_w + 26.0;
        ink.text(R::new(tx, r.y, r.right() - tx - 6.0 - right_w, r.h), row.title.clone(), Font::Body, Ink::Text, Align::Left);
        if row.joinable {
            ink.join(r.right() - 6.0, r.y + 4.0, 24.0, Hit::Join(i));
        } else if !row.right.is_empty() {
            ink.text(R::new(r.right() - 62.0, r.y, 56.0, r.h), row.right.clone(), Font::Caption, Ink::Text3, Align::Right);
        }
    }
    ink.draw(area, Draw::Unclip);
    scrollbar(ink, area, input.rows.len() as f32 * ROW_H, scroll);

    footer(ink, card, '\u{E787}', words().date_settings);
    Some((area, max))
}

/// A day cell's height and its circle's size, for the grid.
pub(super) fn cell_size(grid: Grid) -> (f32, f32) {
    match grid {
        Grid::Compact => (32.0, 24.0),
        _ => (40.0, 30.0),
    }
}

/// The grid's height (its title, the weekdays, the days).
pub(super) fn grid_height(input: &Input) -> f32 {
    if input.grid == Grid::None {
        return 0.0;
    }
    let (cell, _) = cell_size(input.grid);
    MONTH_TITLE_H + WEEKDAYS_H + WEEKS * cell + GRID_GAP
}

/// The month's days from `y`, with week numbers when asked. Returns where it ends.
pub(super) fn days_grid(ink: &mut Layout, card: R, input: &Input, mut y: f32) -> f32 {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    ink.text(R::new(x, y, w - 72.0, 28.0), input.month_title.clone(), Font::Strong, Ink::Text, Align::Left);
    let next = R::new(card.right() - PAD - 28.0, y, 28.0, 28.0);
    let prev = R::new(next.x - 32.0, y, 28.0, 28.0);
    ink.plate(prev, Hit::Prev, prev.h / 2.0);
    ink.glyph(prev, '\u{E76B}', Font::IconSmall, Ink::Text2);
    ink.plate(next, Hit::Next, next.h / 2.0);
    ink.glyph(next, '\u{E76C}', Font::IconSmall, Ink::Text2);
    y += MONTH_TITLE_H;
    // Week numbers in a narrow column of their own, before Monday (or the week's first day).
    let numbers = if input.week_numbers.is_empty() { 0.0 } else { 22.0 };
    let gx = x + numbers;
    let col = (w - numbers) / 7.0;
    for (i, head) in input.heads.iter().enumerate() {
        ink.text(R::new(gx + i as f32 * col, y, col, 18.0), head.clone(), Font::Tiny, Ink::Text3, Align::Center);
    }
    y += WEEKDAYS_H;
    let (cell_h, d) = cell_size(input.grid);
    for (row, number) in input.week_numbers.iter().enumerate() {
        ink.text(R::new(x, y + row as f32 * cell_h + 2.0, numbers - 4.0, d), number.to_string(), Font::Tiny, Ink::Text3, Align::Left);
    }
    for (i, cell) in input.days.iter().enumerate() {
        let r = R::new(gx + (i % 7) as f32 * col, y + (i / 7) as f32 * cell_h, col, cell_h);
        let circle = R::new(r.x + (r.w - d) / 2.0, r.y + 2.0, d, d);
        if cell.today {
            ink.draw(circle, Draw::Rect { fill: Fill::Accent, radius: d / 2.0 });
        } else if ink.lit(Hit::Day(cell.day)) {
            ink.draw(circle, Draw::Rect { fill: Fill::Hover, radius: d / 2.0 });
        }
        if cell.selected && !cell.today {
            ink.draw(circle, Draw::Outline { ink: Ink::Accent, radius: d / 2.0 });
        }
        let color = if cell.today { Ink::OnAccent } else if cell.dim { Ink::Text3 } else { Ink::Text };
        ink.text(circle, cell.label.clone(), Font::Body, color, Align::Center);
        if cell.busy > 0 {
            let bw = if cell.busy > 1 { 10.0 } else { 4.0 };
            let mark = R::new(r.x + (r.w - bw) / 2.0, (circle.bottom() + 3.0).min(r.bottom() - 4.0), bw, 4.0);
            ink.draw(mark, Draw::Rect { fill: if cell.today { Fill::Accent } else { Fill::Mark }, radius: 2.0 });
        }
        ink.hit(r, Hit::Day(cell.day));
    }
    y + WEEKS * cell_h + GRID_GAP
}

/// One event's details, in place of the calendar.
pub(super) fn detail_card(ink: &mut Layout, card: R, d: &Detail) {
    let x = card.x + PAD;
    let w = card.w - 2.0 * PAD;
    let mut y = card.y + 12.0;
    let back = R::new(x - 8.0, y, 32.0, 32.0);
    ink.plate(back, Hit::Back, back.h / 2.0);
    ink.glyph(back, '\u{E72B}', Font::IconSmall, Ink::Text);
    ink.text(R::new(x + 28.0, y, w - 28.0, 32.0), d.back.clone(), Font::Caption, Ink::Text2, Align::Left);
    y += 44.0;

    // The title whole, on two lines when it needs them.
    let two = text_w(&d.title, Font::Title) > w - 16.0;
    let title_h = if two { 48.0 } else { 24.0 };
    ink.draw(R::new(x, y + 2.0, 4.0, 38.0 + title_h), Draw::Rect { fill: Fill::Rgb(d.color), radius: 2.0 });
    ink.text(R::new(x + 16.0, y, w - 16.0, title_h), d.title.clone(), if two { Font::TitleWrap } else { Font::Title }, Ink::Text, Align::Left);
    ink.text(R::new(x + 16.0, y + title_h + 2.0, w - 16.0, 20.0), d.when.clone(), Font::Body, Ink::Text2, Align::Left);
    ink.text(R::new(x + 16.0, y + title_h + 22.0, w - 16.0, 18.0), d.soon.clone(), Font::Caption, Ink::Accent, Align::Left);
    y += 58.0 + title_h;

    let line = |ink: &mut Layout, y: &mut f32, glyph: char, text: &str, h: f32| {
        ink.glyph(R::new(x, *y, 16.0, 20.0), glyph, Font::IconSmall, Ink::Text2);
        ink.text(R::new(x + 30.0, *y, w - 30.0, h), text, if h > 20.0 { Font::Wrap } else { Font::Body }, Ink::Text, Align::Left);
        *y += h + 10.0;
    };
    if !d.place.is_empty() {
        line(ink, &mut y, '\u{E707}', &d.place, 20.0);
        // A place one can go to, not an online meeting's.
        if d.directions {
            let dir = R::new(x + 26.0, y - 8.0, text_w(words().directions, Font::Caption) + 12.0, 20.0);
            ink.plate(dir, Hit::Directions, 4.0);
            ink.text(R::new(dir.x + 4.0, dir.y, dir.w, dir.h), words().directions, Font::Caption, Ink::Accent, Align::Left);
            y += 16.0;
        }
    }
    if !d.people.is_empty() {
        ink.glyph(R::new(x, y + 2.0, 16.0, 24.0), '\u{E716}', Font::IconSmall, Ink::Text2);
        let mut px = x + 30.0;
        for initials in d.initials.iter().take(3) {
            let c = R::new(px, y, 26.0, 26.0);
            ink.draw(c, Draw::Rect { fill: Fill::Raised, radius: 13.0 });
            ink.text(c, initials.clone(), Font::Tiny, Ink::Text, Align::Center);
            px += 30.0;
        }
        ink.text(R::new(px + 4.0, y, x + w - px - 4.0, 26.0), d.people.clone(), Font::Caption, Ink::Text2, Align::Left);
        y += 36.0;
    }
    let buttons_y = card.bottom() - PAD - 34.0;
    if !d.notes.is_empty() {
        let h = (buttons_y - 70.0 - y).clamp(20.0, 120.0);
        line(ink, &mut y, '\u{E70B}', &d.notes, h);
    }
    line(ink, &mut y, '\u{E787}', &d.source, 20.0);

    let mut bx = x;
    if d.joinable {
        let r = R::new(bx, buttons_y, (text_w(words().join, Font::CaptionStrong) + 40.0).max(96.0), 34.0);
        let fill = ink.accent(Hit::DetailJoin);
        ink.draw(r, Draw::Rect { fill, radius: 4.0 });
        ink.text(r, words().join, Font::CaptionStrong, Ink::OnAccent, Align::Center);
        ink.hit(r, Hit::DetailJoin);
        bx += r.w + 8.0;
    }
    if let Some(open) = &d.open {
        let r = R::new(bx, buttons_y, x + w - bx, 34.0);
        let fill = ink.surface(Hit::DetailOpen, Fill::Raised);
        ink.draw(r, Draw::Rect { fill, radius: 4.0 });
        ink.text(r, open.clone(), Font::Caption, Ink::Text, Align::Center);
        ink.hit(r, Hit::DetailOpen);
    }
}
