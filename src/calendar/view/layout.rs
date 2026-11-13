//! Where everything in the calendar sits, in DIPs: pure, so it can be tested.

use crate::calendar::time;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct R {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl R {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        R { x, y, w, h }
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.w && y >= self.y && y < self.y + self.h
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    #[cfg(test)]
    pub fn center(&self) -> (f32, f32) {
        (self.x + self.w / 2.0, self.y + self.h / 2.0)
    }
}

pub const W: f32 = 360.0;
pub const PAD: f32 = 16.0;
const HEADER_H: f32 = 52.0;
const MONTH_H: f32 = 44.0;
const WEEKDAYS_H: f32 = 28.0;
pub const CELL_H: f32 = 42.0;
const AGENDA_HEAD_H: f32 = 40.0;
pub const ROW_H: f32 = 52.0;
const FOOTER_H: f32 = 40.0;
/// The agenda shows this many rows, fewer on a short screen.
pub const ROWS_MAX: usize = 5;
pub const ROWS_MIN: usize = 2;
pub const BUTTON: f32 = 32.0;
/// The join button on an agenda row.
const JOIN_W: f32 = 52.0;
const JOIN_H: f32 = 26.0;

/// Everything with a place.
pub struct Layout {
    pub w: f32,
    pub h: f32,
    /// The day's date, which also goes back to today.
    pub date: R,
    pub bell: R,
    pub gear: R,
    pub rule1: f32,
    pub month: R,
    pub prev: R,
    pub next: R,
    pub weekdays: [R; 7],
    /// 6 weeks of 7 days.
    pub cells: Vec<R>,
    pub rule2: f32,
    pub agenda_title: R,
    /// The rows' viewport: they scroll inside it.
    pub agenda: R,
    pub footer: R,
}

/// The fixed part of the height: everything but the agenda's rows.
fn fixed_height() -> f32 {
    HEADER_H + 1.0 + MONTH_H + WEEKDAYS_H + 6.0 * CELL_H + 8.0 + 1.0 + AGENDA_HEAD_H + FOOTER_H
}

/// How many agenda rows fit in `room` DIPs of height.
pub fn rows_for(room: f32) -> usize {
    (((room - fixed_height()) / ROW_H).floor().max(0.0) as usize).clamp(ROWS_MIN, ROWS_MAX)
}

pub fn layout(rows: usize) -> Layout {
    let inner = W - 2.0 * PAD;
    let mut y = 0.0;
    let date = R::new(PAD, y, inner - 2.0 * BUTTON - 8.0, HEADER_H);
    let gear = R::new(W - PAD - BUTTON, y + (HEADER_H - BUTTON) / 2.0, BUTTON, BUTTON);
    let bell = R::new(gear.x - BUTTON - 4.0, gear.y, BUTTON, BUTTON);
    y += HEADER_H;
    let rule1 = y;
    y += 1.0;
    let month = R::new(PAD, y, inner - 2.0 * BUTTON, MONTH_H);
    let next = R::new(W - PAD - BUTTON, y + (MONTH_H - BUTTON) / 2.0, BUTTON, BUTTON);
    let prev = R::new(next.x - BUTTON, next.y, BUTTON, BUTTON);
    y += MONTH_H;
    let col = inner / 7.0;
    let weekdays = std::array::from_fn(|i| R::new(PAD + i as f32 * col, y, col, WEEKDAYS_H));
    y += WEEKDAYS_H;
    let cells = (0..42).map(|i| R::new(PAD + (i % 7) as f32 * col, y + (i / 7) as f32 * CELL_H, col, CELL_H)).collect();
    y += 6.0 * CELL_H + 8.0;
    let rule2 = y;
    y += 1.0;
    let agenda_title = R::new(PAD, y, inner, AGENDA_HEAD_H);
    y += AGENDA_HEAD_H;
    let agenda = R::new(0.0, y, W, rows as f32 * ROW_H);
    y += agenda.h;
    let footer = R::new(PAD, y, inner, FOOTER_H);
    y += FOOTER_H;
    Layout { w: W, h: y, date, bell, gear, rule1, month, prev, next, weekdays, cells, rule2, agenda_title, agenda, footer }
}

impl Layout {
    /// Agenda row `i`, scrolled.
    pub fn row(&self, i: usize, scroll: f32) -> R {
        R::new(self.agenda.x + 8.0, self.agenda.y + i as f32 * ROW_H - scroll, self.agenda.w - 16.0, ROW_H)
    }

    /// The join button on row `r`.
    pub fn join(&self, r: R) -> R {
        R::new(r.right() - 8.0 - JOIN_W, r.y + (r.h - JOIN_H) / 2.0, JOIN_W, JOIN_H)
    }

    /// How far the agenda scrolls at most for `n` rows.
    pub fn max_scroll(&self, n: usize) -> f32 {
        (n as f32 * ROW_H - self.agenda.h).max(0.0)
    }
}

/// What a click lands on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    /// Back to today.
    Today,
    /// Windows' notifications.
    Bell,
    /// Settings, on the calendar's connector.
    Settings,
    Prev,
    Next,
    /// A day in the grid, by cell.
    Day(usize),
    /// An event in the agenda, by row.
    Event(usize),
    /// Its join button.
    Join(usize),
}

/// The target at `(x, y)`: `rows` agenda rows, `joinable(i)` saying which have a join button.
pub fn hit(l: &Layout, x: f32, y: f32, scroll: f32, rows: usize, joinable: &dyn Fn(usize) -> bool) -> Option<Hit> {
    let simple = [(l.date, Hit::Today), (l.bell, Hit::Bell), (l.gear, Hit::Settings), (l.prev, Hit::Prev), (l.next, Hit::Next)];
    if let Some((_, hit)) = simple.iter().find(|(r, _)| r.contains(x, y)) {
        return Some(*hit);
    }
    if let Some(i) = l.cells.iter().position(|c| c.contains(x, y)) {
        return Some(Hit::Day(i));
    }
    if l.agenda.contains(x, y) {
        let i = ((y - l.agenda.y + scroll) / ROW_H).floor() as usize;
        if i < rows {
            if joinable(i) && l.join(l.row(i, scroll)).contains(x, y) {
                return Some(Hit::Join(i));
            }
            return Some(Hit::Event(i));
        }
    }
    None
}

/// The first day the grid shows for month `(y, m)`: the week's first day on or before the 1st.
pub fn grid_start(y: i32, m: u32, first_weekday: u32) -> i32 {
    let first = time::days_from_civil(y, m, 1);
    first - ((time::weekday(first) + 7 - first_weekday) % 7) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::time::days_from_civil;

    #[test]
    fn the_grid_starts_on_the_locales_first_weekday() {
        // November 2026 starts on a Sunday.
        assert_eq!(grid_start(2026, 11, 0), days_from_civil(2026, 10, 26), "Monday first");
        assert_eq!(grid_start(2026, 11, 6), days_from_civil(2026, 11, 1), "Sunday first");
        // February 2027 starts on a Monday: no days of January.
        assert_eq!(grid_start(2027, 2, 0), days_from_civil(2027, 2, 1));
    }

    #[test]
    fn everything_fits_and_each_part_is_hit_where_it_is() {
        let l = layout(4);
        assert!(l.cells.iter().all(|c| c.right() <= W - PAD + 0.01));
        assert_eq!(l.footer.bottom(), l.h);
        let none = |_: usize| false;
        let (cx, cy) = l.cells[12].center();
        assert_eq!(hit(&l, cx, cy, 0.0, 0, &none), Some(Hit::Day(12)));
        let (px, py) = l.prev.center();
        assert_eq!(hit(&l, px, py, 0.0, 0, &none), Some(Hit::Prev));
        let row = l.row(1, 0.0);
        assert_eq!(hit(&l, row.x + 20.0, row.y + 10.0, 0.0, 3, &none), Some(Hit::Event(1)));
        assert_eq!(hit(&l, row.x + 20.0, row.y + 10.0, 0.0, 1, &none), None, "no row there");
        let join = l.join(row);
        assert_eq!(hit(&l, join.x + 4.0, join.y + 4.0, 0.0, 3, &|i| i == 1), Some(Hit::Join(1)));
        // Scrolled by a row, the same place is the next event.
        assert_eq!(hit(&l, row.x + 20.0, row.y + 10.0, ROW_H, 3, &none), Some(Hit::Event(2)));
    }

    #[test]
    fn the_agenda_shrinks_on_short_screens() {
        assert_eq!(rows_for(2000.0), ROWS_MAX);
        assert_eq!(rows_for(100.0), ROWS_MIN);
        assert_eq!(layout(3).h - layout(2).h, ROW_H);
    }
}
