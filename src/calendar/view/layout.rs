//! Where everything in the calendar sits, in DIPs: pure, so it can be tested.
//!
//! Top to bottom: Windows' notifications (when there are any and they are wanted), the day's date
//! with the world clocks under it, the month (unless the agenda stands alone), the selected day's
//! agenda (or one event's details), and the footer with the focus timer.

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
const CLOCK_H: f32 = 22.0;
const MONTH_H: f32 = 44.0;
const WEEKDAYS_H: f32 = 28.0;
const CELL_H: f32 = 42.0;
const CELL_H_COMPACT: f32 = 32.0;
/// The week numbers' column.
const WEEK_W: f32 = 28.0;
const AGENDA_HEAD_H: f32 = 40.0;
pub const ROW_H: f32 = 52.0;
const FOOTER_H: f32 = 48.0;
const TOAST_HEAD_H: f32 = 40.0;
/// In the notification centre: an app's heading, a notification, and the line saying there are none.
pub const GROUP_H: f32 = 30.0;
pub const TOAST_H: f32 = 58.0;
pub const EMPTY_H: f32 = 36.0;
/// The centre's list takes at most this much height; it scrolls past it.
pub const CENTER_MAX: f32 = 380.0;
pub const ROWS_MIN: usize = 2;
pub const BUTTON: f32 = 32.0;
const JOIN_W: f32 = 52.0;
const JOIN_H: f32 = 26.0;

/// What the layout depends on besides the screen.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Shape {
    /// Agenda rows wanted (fewer on a short screen).
    pub rows: usize,
    pub weeks: bool,
    pub compact: bool,
    pub agenda_only: bool,
    /// Notifications listed (0: no section).
    /// The notification centre's list, its height (0: no centre).
    pub center: f32,
    /// World clocks under the date.
    pub clocks: usize,
}

/// Everything with a place.
pub struct Layout {
    pub w: f32,
    pub h: f32,
    /// The notifications' section: its title, Clear all, and its rows.
    pub toast_title: Option<R>,
    pub clear: Option<R>,
    /// The list's viewport: its rows scroll inside it.
    pub center: Option<R>,
    /// The day's date, which also goes back to today.
    pub date: R,
    pub bell: R,
    pub gear: R,
    /// A row per world clock.
    pub clocks: Vec<R>,
    pub rule1: f32,
    /// The month; `None` when the agenda stands alone.
    pub month: Option<R>,
    pub prev: R,
    pub next: R,
    pub weekdays: [R; 7],
    /// The week number of each grid row.
    pub weeks: Vec<R>,
    /// 6 weeks of 7 days (empty when the agenda stands alone).
    pub cells: Vec<R>,
    pub rule2: f32,
    pub agenda_title: R,
    /// The day before and after, when the agenda stands alone.
    pub day_prev: Option<R>,
    pub day_next: Option<R>,
    /// The rows' viewport: they scroll inside it. An event's details take it over.
    pub agenda: R,
    pub footer: R,
    pub focus_less: R,
    pub focus_more: R,
    pub focus_button: R,
}

/// The height of everything but the agenda's rows.
fn fixed_height(shape: Shape) -> f32 {
    let toasts = if shape.center > 0.0 { TOAST_HEAD_H + shape.center + 1.0 } else { 0.0 };
    let clocks = if shape.clocks > 0 { shape.clocks as f32 * CLOCK_H + 8.0 } else { 0.0 };
    let month = if shape.agenda_only { 0.0 } else { MONTH_H + WEEKDAYS_H + 6.0 * cell_h(shape) + 8.0 + 1.0 };
    toasts + HEADER_H + clocks + 1.0 + month + AGENDA_HEAD_H + FOOTER_H
}

fn cell_h(shape: Shape) -> f32 {
    if shape.compact { CELL_H_COMPACT } else { CELL_H }
}

/// How many agenda rows fit in `room` DIPs of height.
pub fn rows_for(shape: Shape, room: f32) -> usize {
    let fit = ((room - fixed_height(shape)) / ROW_H).floor().max(0.0) as usize;
    fit.min(shape.rows).max(ROWS_MIN)
}

pub fn layout(shape: Shape) -> Layout {
    let inner = W - 2.0 * PAD;
    let mut y = 0.0;

    let (mut toast_title, mut clear, mut center) = (None, None, None);
    if shape.center > 0.0 {
        toast_title = Some(R::new(PAD, y, inner - 96.0, TOAST_HEAD_H));
        clear = Some(R::new(W - PAD - 88.0, y + 6.0, 88.0, 28.0));
        y += TOAST_HEAD_H;
        center = Some(R::new(0.0, y, W, shape.center));
        y += shape.center;
        y += 1.0;
    }

    let date = R::new(PAD, y, inner - 2.0 * BUTTON - 8.0, HEADER_H);
    let gear = R::new(W - PAD - BUTTON, y + (HEADER_H - BUTTON) / 2.0, BUTTON, BUTTON);
    let bell = R::new(gear.x - BUTTON - 4.0, gear.y, BUTTON, BUTTON);
    y += HEADER_H;
    let mut clocks = Vec::new();
    if shape.clocks > 0 {
        for _ in 0..shape.clocks {
            clocks.push(R::new(PAD, y, inner, CLOCK_H));
            y += CLOCK_H;
        }
        y += 8.0;
    }
    let rule1 = y;
    y += 1.0;

    let (mut month, mut weeks, mut cells) = (None, Vec::new(), Vec::new());
    let mut weekdays = [R::default(); 7];
    let mut prev = R::default();
    let mut next = R::default();
    let mut rule2 = y;
    if !shape.agenda_only {
        month = Some(R::new(PAD, y, inner - 2.0 * BUTTON, MONTH_H));
        next = R::new(W - PAD - BUTTON, y + (MONTH_H - BUTTON) / 2.0, BUTTON, BUTTON);
        prev = R::new(next.x - BUTTON, next.y, BUTTON, BUTTON);
        y += MONTH_H;
        let left = PAD + if shape.weeks { WEEK_W } else { 0.0 };
        let col = (W - PAD - left) / 7.0;
        weekdays = std::array::from_fn(|i| R::new(left + i as f32 * col, y, col, WEEKDAYS_H));
        y += WEEKDAYS_H;
        let ch = cell_h(shape);
        cells = (0..42).map(|i| R::new(left + (i % 7) as f32 * col, y + (i / 7) as f32 * ch, col, ch)).collect();
        if shape.weeks {
            weeks = (0..6).map(|row| R::new(PAD, y + row as f32 * ch, WEEK_W, ch)).collect();
        }
        y += 6.0 * ch + 8.0;
        rule2 = y;
        y += 1.0;
    }

    let agenda_title = R::new(PAD, y, inner - if shape.agenda_only { 2.0 * BUTTON } else { 0.0 }, AGENDA_HEAD_H);
    let (day_prev, day_next) = if shape.agenda_only {
        let n = R::new(W - PAD - BUTTON, y + (AGENDA_HEAD_H - BUTTON) / 2.0, BUTTON, BUTTON);
        (Some(R::new(n.x - BUTTON, n.y, BUTTON, BUTTON)), Some(n))
    } else {
        (None, None)
    };
    y += AGENDA_HEAD_H;
    let agenda = R::new(0.0, y, W, shape.rows as f32 * ROW_H);
    y += agenda.h;
    let footer = R::new(PAD, y, inner, FOOTER_H);
    let focus_button = R::new(W - PAD - 92.0, y + (FOOTER_H - 30.0) / 2.0, 92.0, 30.0);
    let focus_more = R::new(focus_button.x - 8.0 - 28.0, focus_button.y + 1.0, 28.0, 28.0);
    let focus_less = R::new(focus_more.x - 66.0, focus_more.y, 28.0, 28.0);
    y += FOOTER_H;
    Layout {
        w: W,
        h: y,
        toast_title,
        clear,
        center,
        date,
        bell,
        gear,
        clocks,
        rule1,
        month,
        prev,
        next,
        weekdays,
        weeks,
        cells,
        rule2,
        agenda_title,
        day_prev,
        day_next,
        agenda,
        footer,
        focus_less,
        focus_more,
        focus_button,
    }
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

    /// The dismiss cross on a notification centre row `r`.
    pub fn dismiss(&self, r: R) -> R {
        R::new(r.right() - 32.0, r.y + (r.h - 28.0) / 2.0, 28.0, 28.0)
    }

    /// An event's details, inside the agenda: the back button, then two buttons at the bottom.
    pub fn back(&self) -> R {
        R::new(self.agenda_title.x - 8.0, self.agenda_title.y + (self.agenda_title.h - BUTTON) / 2.0, BUTTON, BUTTON)
    }

    pub fn detail_buttons(&self) -> (R, R) {
        let y = self.agenda.bottom() - 40.0;
        let join = R::new(PAD, y, 120.0, 30.0);
        (join, R::new(join.right() + 8.0, y, 120.0, 30.0))
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
    /// Do not disturb on or off.
    Quiet,
    /// Settings, on the Calendar page.
    Settings,
    Prev,
    Next,
    /// A day in the grid, by cell.
    Day(usize),
    /// The day before or after (the agenda alone).
    DayPrev,
    DayNext,
    /// An event in the agenda, by row: opens its details.
    Event(usize),
    /// Its join button.
    Join(usize),
    /// Out of an event's details.
    Back,
    DetailJoin,
    DetailOpen,
    /// The world clocks: Windows' settings for them.
    Clocks,
    ClearAll,
    /// A notification (by its place in the centre's list): opens its app.
    Toast(usize),
    Dismiss(usize),
    /// An app's heading's cross: dismiss all of its notifications.
    ClearApp(usize),
    FocusLess,
    FocusMore,
    /// Start or stop the focus timer.
    Focus,
}

/// What is on screen besides the layout, for hit-testing.
pub struct Showing<'a> {
    pub scroll: f32,
    pub rows: usize,
    pub joinable: &'a dyn Fn(usize) -> bool,
    /// An event's details are open: whether it can be joined and opened.
    pub detail: Option<(bool, bool)>,
}

/// The target at `(x, y)`.
pub fn hit(l: &Layout, x: f32, y: f32, s: &Showing) -> Option<Hit> {
    let mut simple = vec![(l.date, Hit::Today), (l.bell, Hit::Quiet), (l.gear, Hit::Settings), (l.focus_less, Hit::FocusLess), (l.focus_more, Hit::FocusMore), (l.focus_button, Hit::Focus)];
    if l.month.is_some() {
        simple.extend([(l.prev, Hit::Prev), (l.next, Hit::Next)]);
    }
    if let (Some(p), Some(n)) = (l.day_prev, l.day_next) {
        simple.extend([(p, Hit::DayPrev), (n, Hit::DayNext)]);
    }
    if let Some(c) = l.clear {
        simple.push((c, Hit::ClearAll));
    }
    simple.extend(l.clocks.iter().map(|r| (*r, Hit::Clocks)));
    if let Some((join, open)) = s.detail {
        simple.push((l.back(), Hit::Back));
        let (j, o) = l.detail_buttons();
        if join {
            simple.push((j, Hit::DetailJoin));
        }
        if open {
            simple.push((if join { o } else { j }, Hit::DetailOpen));
        }
    }
    if let Some((_, hit)) = simple.iter().find(|(r, _)| r.contains(x, y)) {
        return Some(*hit);
    }
    if let Some(i) = l.cells.iter().position(|c| c.contains(x, y)) {
        return Some(Hit::Day(i));
    }
    if s.detail.is_none() && l.agenda.contains(x, y) {
        let i = ((y - l.agenda.y + s.scroll) / ROW_H).floor() as usize;
        if i < s.rows {
            if (s.joinable)(i) && l.join(l.row(i, s.scroll)).contains(x, y) {
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

/// The ISO 8601 week number of a day (weeks start on Monday; week 1 holds the year's first
/// Thursday).
pub fn iso_week(day: i32) -> u32 {
    let thursday = day - time::weekday(day) as i32 + 3;
    let (y, _, _) = time::civil_from_days(thursday);
    ((thursday - time::days_from_civil(y, 1, 1)) / 7 + 1) as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::calendar::time::days_from_civil;

    fn shape() -> Shape {
        Shape { rows: 4, weeks: false, compact: false, agenda_only: false, center: 0.0, clocks: 0 }
    }

    fn showing<'a>(rows: usize, joinable: &'a dyn Fn(usize) -> bool) -> Showing<'a> {
        Showing { scroll: 0.0, rows, joinable, detail: None }
    }

    #[test]
    fn the_grid_starts_on_the_locales_first_weekday() {
        assert_eq!(grid_start(2026, 11, 0), days_from_civil(2026, 10, 26), "Monday first");
        assert_eq!(grid_start(2026, 11, 6), days_from_civil(2026, 11, 1), "Sunday first");
        assert_eq!(grid_start(2027, 2, 0), days_from_civil(2027, 2, 1));
    }

    #[test]
    fn iso_weeks_cross_the_year_correctly() {
        assert_eq!(iso_week(days_from_civil(2026, 11, 13)), 46);
        assert_eq!(iso_week(days_from_civil(2027, 1, 1)), 53, "a Friday: still 2026's last week");
        assert_eq!(iso_week(days_from_civil(2026, 1, 1)), 1, "a Thursday");
        assert_eq!(iso_week(days_from_civil(2024, 12, 30)), 1, "a Monday of 2025's first week");
    }

    #[test]
    fn everything_fits_and_each_part_is_hit_where_it_is() {
        let l = layout(shape());
        assert!(l.cells.iter().all(|c| c.right() <= W - PAD + 0.01));
        assert_eq!(l.footer.bottom(), l.h);
        let none = |_: usize| false;
        let (cx, cy) = l.cells[12].center();
        assert_eq!(hit(&l, cx, cy, &showing(0, &none)), Some(Hit::Day(12)));
        let (px, py) = l.prev.center();
        assert_eq!(hit(&l, px, py, &showing(0, &none)), Some(Hit::Prev));
        let row = l.row(1, 0.0);
        assert_eq!(hit(&l, row.x + 20.0, row.y + 10.0, &showing(3, &none)), Some(Hit::Event(1)));
        assert_eq!(hit(&l, row.x + 20.0, row.y + 10.0, &showing(1, &none)), None, "no row there");
        let join = l.join(row);
        assert_eq!(hit(&l, join.x + 4.0, join.y + 4.0, &showing(3, &|i| i == 1)), Some(Hit::Join(1)));
        let (fx, fy) = l.focus_button.center();
        assert_eq!(hit(&l, fx, fy, &showing(0, &none)), Some(Hit::Focus));
    }

    #[test]
    fn sections_come_and_go_with_what_there_is() {
        let base = layout(shape());
        let full = layout(Shape { center: 120.0, clocks: 2, weeks: true, ..shape() });
        assert_eq!(full.h - base.h, TOAST_HEAD_H + 120.0 + 1.0 + 2.0 * CLOCK_H + 8.0);
        assert_eq!(full.center.map(|c| c.h), Some(120.0));
        assert_eq!(full.weeks.len(), 6);
        assert!(full.cells[0].x >= PAD + WEEK_W);
        let alone = layout(Shape { agenda_only: true, ..shape() });
        assert!(alone.cells.is_empty() && alone.month.is_none() && alone.day_next.is_some());
        assert!(alone.h < base.h);
        let compact = layout(Shape { compact: true, ..shape() });
        assert_eq!(base.h - compact.h, 6.0 * (CELL_H - CELL_H_COMPACT));
    }

    #[test]
    fn the_agenda_shrinks_on_short_screens() {
        assert_eq!(rows_for(shape(), 2000.0), 4);
        assert_eq!(rows_for(shape(), 100.0), ROWS_MIN);
    }
}
