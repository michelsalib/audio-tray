//! The calendar flyout: Windows' notifications, the day's date and world clocks, a month grid
//! with a dot per day that has events, the selected day's agenda (or one event's details), and the
//! focus timer. It opens over the clock's corner, on whichever edge the taskbar is, slides in like
//! the shell's own flyouts, and closes on a click elsewhere, Escape, or losing the foreground.
//!
//! A per-pixel-alpha layered window over Windows' acrylic, drawn off screen with Direct2D
//! ([`paint`]) and presented whole on every change: no `WM_PAINT`.
//!
//! **Re-entrancy.** Opening a link or Settings can pump messages: that work runs after the view's
//! state is released ([`After`]).

mod layout;
mod locale;
mod paint;

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{GetMonitorInfoW, MonitorFromPoint, ScreenToClient, MONITORINFO, MONITOR_DEFAULTTONEAREST};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SendInput, SetCapture, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_DOWN, VK_ESCAPE, VK_HOME, VK_LEFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, ShowWindow, SW_SHOWNA, WA_INACTIVE, WHEEL_DELTA, WINDOW_EX_STYLE, WM_ACTIVATE,
    WM_CAPTURECHANGED, WM_DESTROY, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_RBUTTONDOWN,
    WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
};

use super::model::{self, Event, When};
use super::notifications::Toast;
use super::time::{self, Local, Zone};
use super::zones::WorldClock;
use super::Setup;
use crate::config::{CalendarLayout, Config};
use layout::{Hit, Layout, Shape, Showing, R};
use paint::{Align, Color, Font, Gfx, Palette, Pen};

const CLASS: PCWSTR = w!("AudioTrayCalendar");
/// A click on the clock right after the calendar closed is the click that closed it.
const SETTLE: Duration = Duration::from_millis(400);
/// The gap between the calendar and the screen's edges, as the shell's flyouts keep.
const MARGIN: f32 = 12.0;
/// How far a wheel notch scrolls the agenda.
const SCROLL_STEP: f32 = 48.0;
/// The focus timer's steps and bounds, in minutes.
const FOCUS_STEP: u32 = 5;
const FOCUS_MIN: u32 = 5;
const FOCUS_MAX: u32 = 240;

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    static OPEN: Cell<isize> = const { Cell::new(0) };
    static CLOSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The close animation is running.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
}

/// The screen edge the taskbar is on, where the calendar opens from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Edge {
    Bottom,
    Top,
    Left,
    Right,
}

struct View {
    hwnd: HWND,
    gfx: RefCell<Gfx>,
    palette: Palette,
    scale: f32,
    edge: Edge,
    /// The work area it sits in, in pixels.
    work: RECT,
    shape: Shape,
    layout: Layout,
    /// Where it sits, and its size, in pixels.
    pos: (i32, i32),
    size: (i32, i32),
    config: Config,
    setup: Setup,
    has_sources: bool,
    today: i32,
    month: (i32, u32),
    selected: i32,
    first_weekday: u32,
    /// Short day names, Monday first.
    weekdays: [String; 7],
    /// The shown calendars' events, in their chosen colours.
    events: Vec<Event>,
    /// The selected day's events.
    agenda: Vec<Event>,
    /// The agenda row whose details are open.
    detail: Option<usize>,
    scroll: f32,
    hover: Option<Hit>,
    pressed: Option<Hit>,
    toasts: Vec<Toast>,
    clocks: Vec<WorldClock>,
    focus_minutes: u32,
}

/// Work to do once the view's state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

/// Open the calendar, or close it if it is open.
pub fn toggle(config: &Config) {
    if OPEN.with(Cell::get) != 0 {
        close();
        return;
    }
    if CLOSED_AT.with(Cell::get).is_some_and(|at| at.elapsed() < SETTLE) {
        return;
    }
    if let Err(e) = create(config) {
        eprintln!("calendar: could not open ({e:#})");
    }
}

pub fn close() {
    let open = OPEN.with(Cell::get);
    if open == 0 || CLOSING.with(|c| c.replace(true)) {
        return;
    }
    let hwnd = HWND(open as *mut _);
    with_view(|v| v.animate(crate::win::Flyout::Close));
    let _ = unsafe { DestroyWindow(hwnd) };
    CLOSING.with(|c| c.set(false));
}

/// The events or the notifications changed: show them.
pub fn refresh() {
    with_view(|v| {
        v.reload();
        v.relayout();
    });
}

fn with_view<R>(f: impl FnOnce(&mut View) -> R) -> Option<R> {
    VIEW.with(|cell| cell.try_borrow_mut().ok().and_then(|mut v| v.as_mut().map(f)))
}

/// The monitor under `point`: its scale, its rectangle and its work area.
fn monitor(point: POINT) -> (f32, RECT, RECT) {
    let (scale, work) = crate::win::monitor_at(point);
    let handle = unsafe { MonitorFromPoint(point, MONITOR_DEFAULTTONEAREST) };
    let mut info = MONITORINFO { cbSize: std::mem::size_of::<MONITORINFO>() as u32, ..Default::default() };
    let rect = if unsafe { GetMonitorInfoW(handle, &mut info) }.as_bool() { info.rcMonitor } else { work };
    (scale, rect, work)
}

/// Which edge the taskbar takes from this monitor (the side its work area is short of).
fn edge_of(rect: &RECT, work: &RECT) -> Edge {
    if work.top > rect.top {
        Edge::Top
    } else if work.left > rect.left {
        Edge::Left
    } else if work.right < rect.right {
        Edge::Right
    } else {
        Edge::Bottom
    }
}

fn empty_shape() -> Shape {
    Shape { rows: 0, weeks: false, compact: false, agenda_only: false, toasts: 0, clocks: 0 }
}

fn create(config: &Config) -> Result<()> {
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let (scale, rect, work) = monitor(cursor);
    let style = WINDOW_EX_STYLE(WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0);
    let hwnd = crate::win::create_popup(CLASS, w!("Calendar"), Some(wndproc), style, (0, 0, 1, 1)).context("CreateWindowExW")?;
    unsafe { crate::layered::style_panel(hwnd, crate::layered::CORNER_ROUND) };
    let today = time::today();
    let (ty, tm, _) = time::civil_from_days(today);
    let setup = config.calendar.setup();
    let first_weekday = config.calendar.first_weekday.map_or_else(locale::first_weekday, |d| d.min(6) as u32);
    let mut view = View {
        hwnd,
        gfx: RefCell::new(Gfx::new()?),
        palette: Palette::current(),
        scale,
        edge: edge_of(&rect, &work),
        work,
        shape: empty_shape(),
        layout: layout::layout(empty_shape()),
        pos: (0, 0),
        size: (1, 1),
        has_sources: super::has_sources(&setup),
        setup,
        config: config.clone(),
        today,
        month: (ty, tm),
        selected: today,
        first_weekday,
        weekdays: locale::weekday_names(),
        events: Vec::new(),
        agenda: Vec::new(),
        detail: None,
        scroll: 0.0,
        hover: None,
        pressed: None,
        toasts: Vec::new(),
        clocks: Vec::new(),
        focus_minutes: config.calendar.focus_minutes.clamp(FOCUS_MIN, FOCUS_MAX),
    };
    view.reload();
    view.fetch();
    view.place();
    view.draw_frame();
    VIEW.with(|cell| *cell.borrow_mut() = Some(view));
    OPEN.with(|o| o.set(hwnd.0 as isize));
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
    with_view(|v| v.animate(crate::win::Flyout::Open));
    // The click that opened it was swallowed by our hook, so Windows has not lent us the
    // foreground: without it there are no keys (Escape) and no deactivation to close on.
    if crate::music::player::raise(hwnd) == crate::music::player::Raise::Refused {
        eprintln!("calendar: could not take the foreground");
    }
    unsafe {
        // Like a menu: a click anywhere else comes here first, and closes it.
        SetCapture(hwnd);
    }
    arrow();
    keep_rect();
    Ok(())
}

/// Tell the mouse hook where the calendar is: a click outside it closes it.
fn keep_rect() {
    let rect = with_view(|v| RECT { left: v.pos.0, top: v.pos.1, right: v.pos.0 + v.size.0, bottom: v.pos.1 + v.size.1 });
    *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner()) = rect;
}

impl View {
    /// Take the cache's events (shown calendars, chosen colours), the selected day's, Windows'
    /// notifications and the world clocks.
    fn reload(&mut self) {
        let calendar = &self.config.calendar;
        self.events = super::events()
            .0
            .into_iter()
            .filter(|e| !calendar.hidden.contains(&e.calendar))
            .map(|mut e| {
                if let Some(color) = calendar.colors.get(&e.calendar).and_then(|c| super::parse_hex(c)) {
                    e.color = color;
                }
                e
            })
            .collect();
        self.agenda = model::on_day(&self.events, self.selected, &Local).into_iter().cloned().collect();
        if self.detail.is_some_and(|i| i >= self.agenda.len()) {
            self.detail = None;
        }
        self.toasts = if calendar.notifications { super::notifications::list() } else { Vec::new() };
        self.clocks = if calendar.world_clocks { super::zones::clocks() } else { Vec::new() };
    }

    fn fetch(&self) {
        if self.has_sources {
            super::fetch(&self.setup, super::days_around(self.month.0, self.month.1), false);
        }
    }

    /// Lay out for what there is now, and put the window in its corner.
    fn place(&mut self) {
        let calendar = &self.config.calendar;
        let mut shape = Shape {
            rows: (calendar.rows as usize).clamp(layout::ROWS_MIN, 12),
            weeks: calendar.week_numbers,
            compact: calendar.layout == CalendarLayout::Compact,
            agenda_only: calendar.layout == CalendarLayout::AgendaOnly,
            toasts: self.toasts.len().min(layout::TOASTS_MAX),
            clocks: self.clocks.len(),
        };
        let room = (self.work.bottom - self.work.top) as f32 / self.scale - 2.0 * MARGIN;
        let most = shape.rows;
        let fit = |shape: Shape| layout::rows_for(Shape { rows: most, ..shape }, room);
        shape.rows = fit(shape);
        // Too tall, or the agenda squeezed under three rows: fewer notifications, then no world
        // clocks.
        let wanted = most.min(3);
        while (layout::layout(shape).h > room || shape.rows < wanted) && (shape.toasts > 0 || shape.clocks > 0) {
            if shape.toasts > 0 {
                shape.toasts -= 1;
            } else {
                shape.clocks = 0;
            }
            shape.rows = fit(shape);
        }
        self.shape = shape;
        self.layout = layout::layout(shape);
        let (w, h) = ((self.layout.w * self.scale).round() as i32, (self.layout.h * self.scale).round() as i32);
        let m = (MARGIN * self.scale).round() as i32;
        let work = self.work;
        self.pos = match self.edge {
            Edge::Bottom | Edge::Right => (work.right - w - m, work.bottom - h - m),
            Edge::Top => (work.right - w - m, work.top + m),
            Edge::Left => (work.left + m, work.bottom - h - m),
        };
        self.size = (w, h);
    }

    /// Lay out again (something came or went) and show it.
    fn relayout(&mut self) {
        self.place();
        self.scroll = self.scroll.min(self.layout.max_scroll(self.agenda.len()));
        self.render();
    }

    /// Draw the frame off screen.
    fn draw_frame(&self) {
        let size = (self.size.0 as u32, self.size.1 as u32);
        if let Err(e) = self.gfx.borrow_mut().frame(size, self.scale, |pen| draw(pen, self)) {
            eprintln!("calendar: could not draw ({e:#})");
        }
    }

    /// Draw and put on screen.
    fn render(&self) {
        self.draw_frame();
        crate::layered::present_premultiplied(self.hwnd, &self.gfx.borrow().pixels, self.size.0, self.size.1, self.pos.0, self.pos.1, 255);
    }

    /// Slide in or out from the taskbar's edge, on the last frame drawn.
    fn animate(&self, motion: crate::win::Flyout) {
        let slide = crate::win::FLYOUT_SLIDE * self.scale;
        let (x, y) = self.pos;
        let gfx = self.gfx.borrow();
        motion.run(slide, |offset, alpha| {
            let (ax, ay) = match self.edge {
                Edge::Bottom => (x, y + offset),
                Edge::Top => (x, y - offset),
                Edge::Left => (x - offset, y),
                Edge::Right => (x + offset, y),
            };
            crate::layered::present_premultiplied(self.hwnd, &gfx.pixels, self.size.0, self.size.1, ax, ay, alpha);
        });
    }

    fn select(&mut self, day: i32) {
        if day != self.selected {
            self.scroll = 0.0;
            self.detail = None;
        }
        self.selected = day;
        self.agenda = model::on_day(&self.events, day, &Local).into_iter().cloned().collect();
        let (y, m, _) = time::civil_from_days(day);
        if (y, m) != self.month {
            self.month = (y, m);
            self.fetch();
        }
    }

    fn show_month(&mut self, delta: i32) {
        self.month = time::add_months(self.month.0, self.month.1, delta);
        self.fetch();
    }

    fn grid_start(&self) -> i32 {
        layout::grid_start(self.month.0, self.month.1, self.first_weekday)
    }

    fn hit(&self, x: i32, y: i32) -> Option<Hit> {
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        let joinable = |i: usize| self.agenda.get(i).is_some_and(|e| e.join.is_some());
        let detail = self.detail.and_then(|i| self.agenda.get(i)).map(|e| (e.join.is_some(), e.link.is_some()));
        layout::hit(&self.layout, x, y, &Showing { scroll: self.scroll, rows: self.agenda.len(), joinable: &joinable, detail })
    }

    fn set_hover(&mut self, hover: Option<Hit>) {
        if self.hover != hover {
            self.hover = hover;
            self.render();
        }
    }

    /// Act on a click; what needs the state released comes back.
    fn activate(&mut self, hit: Hit) -> Option<After> {
        let open = |url: String| -> Option<After> {
            Some(Box::new(move || {
                close();
                crate::win::shell_open(&url, None);
            }))
        };
        match hit {
            Hit::Today => {
                self.today = time::today();
                self.select(self.today);
            }
            Hit::Prev => self.show_month(-1),
            Hit::Next => self.show_month(1),
            Hit::Day(i) => self.select(self.grid_start() + i as i32),
            Hit::DayPrev => self.select(self.selected - 1),
            Hit::DayNext => self.select(self.selected + 1),
            Hit::Bell | Hit::Toast(_) => {
                return Some(Box::new(|| {
                    close();
                    open_notifications();
                }))
            }
            Hit::Settings => {
                return Some(Box::new(|| {
                    close();
                    crate::tray::post(crate::settings::WM_OPEN_SETTINGS, crate::settings::OPEN_CALENDAR, 0);
                }))
            }
            Hit::Clocks => {
                return Some(Box::new(|| {
                    close();
                    super::zones::edit();
                }))
            }
            Hit::Event(i) => {
                if self.agenda.get(i).is_some() {
                    self.detail = Some(i);
                }
            }
            Hit::Join(i) => return open(self.agenda.get(i)?.join.clone()?),
            Hit::Back => self.detail = None,
            Hit::DetailJoin => return open(self.agenda.get(self.detail?)?.join.clone()?),
            Hit::DetailOpen => return open(self.agenda.get(self.detail?)?.link.clone()?),
            Hit::ClearAll => {
                super::notifications::clear();
                self.toasts.clear();
                self.hover = None;
                self.relayout();
                return Some(Box::new(keep_rect));
            }
            Hit::Dismiss(i) => {
                let toast = self.toasts.get(i)?;
                super::notifications::remove(toast.id);
                self.toasts.remove(i);
                self.hover = None;
                self.relayout();
                return Some(Box::new(keep_rect));
            }
            Hit::FocusLess => self.focus_minutes = self.focus_minutes.saturating_sub(FOCUS_STEP).max(FOCUS_MIN),
            Hit::FocusMore => self.focus_minutes = (self.focus_minutes + FOCUS_STEP).min(FOCUS_MAX),
            Hit::Focus => {
                if super::schedule::focus_end().is_some() {
                    super::schedule::stop_focus();
                } else {
                    super::schedule::start_focus(self.focus_minutes);
                }
                // The clock and the timer follow.
                crate::tray::wake_calendar();
            }
        }
        self.render();
        None
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> Option<After> {
        if key == VK_ESCAPE {
            if self.detail.take().is_some() {
                self.render();
                return None;
            }
            return Some(Box::new(close));
        }
        let day = match key {
            VK_LEFT => self.selected - 1,
            VK_RIGHT => self.selected + 1,
            VK_UP => self.selected - 7,
            VK_DOWN => self.selected + 7,
            VK_HOME => time::today(),
            VK_PRIOR | VK_NEXT => {
                let (y, m, d) = time::civil_from_days(self.selected);
                let (y, m) = time::add_months(y, m, if key == VK_PRIOR { -1 } else { 1 });
                time::days_from_civil(y, m, d.min(time::days_in_month(y, m)))
            }
            _ => return None,
        };
        self.select(day);
        self.render();
        None
    }

    fn wheel(&mut self, delta: i32, x: i32, y: i32) {
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if self.layout.agenda.contains(x, y) {
            if self.detail.is_none() {
                let max = self.layout.max_scroll(self.agenda.len());
                self.scroll = (self.scroll - notches * SCROLL_STEP).clamp(0.0, max);
            }
        } else if delta != 0 && self.layout.month.is_some_and(|m| y >= m.y && y < self.layout.rule2) {
            // Over the month, as in the shell's calendar: a notch is a month.
            self.show_month(if delta > 0 { -1 } else { 1 });
        }
        self.render();
    }
}

fn draw(pen: &Pen, v: &View) {
    let p = &v.palette;
    let l = &v.layout;
    pen.clear(p.bg);
    let hovered = |hit: Hit| v.hover == Some(hit);
    let button_bg = |hit: Hit, r: R| {
        if hovered(hit) {
            pen.fill(r, 4.0, if v.pressed == Some(hit) { p.pressed } else { p.hover });
        }
    };

    // Windows' notifications.
    if let Some(title) = l.toast_title {
        pen.text(&format!("Notifications · {}", v.toasts.len()), Font::Strong, title, p.text, Align::Left);
        if let Some(clear) = l.clear {
            button_bg(Hit::ClearAll, clear);
            pen.text("Clear all", Font::Caption, clear, p.text2, Align::Center);
        }
        for (i, r) in l.toasts.iter().enumerate() {
            let Some(toast) = v.toasts.get(i) else { continue };
            let lit = hovered(Hit::Toast(i)) || hovered(Hit::Dismiss(i));
            pen.fill(R::new(r.x, r.y + 2.0, r.w, r.h - 4.0), 6.0, if lit { p.hover } else { p.raised });
            let text_w = r.w - 56.0;
            let when = locale::clock(time::second_of_day(Local.to_local(toast.at)));
            pen.text(&toast.app, Font::CaptionStrong, R::new(r.x + 12.0, r.y + 6.0, text_w - 50.0, 18.0), p.text2, Align::Left);
            pen.text(&when, Font::Caption, R::new(r.x + 12.0, r.y + 6.0, text_w, 18.0), p.text3, Align::Right);
            let line = if toast.body.is_empty() { toast.title.clone() } else { format!("{} · {}", toast.title, toast.body) };
            pen.text(&line, Font::Body, R::new(r.x + 12.0, r.y + 24.0, text_w, 22.0), p.text, Align::Left);
            if lit {
                let x = l.dismiss(*r);
                button_bg(Hit::Dismiss(i), x);
                pen.glyph('\u{E711}', Font::IconSmall, x, p.text2);
            }
        }
        pen.fill(R::new(0.0, l.date.y - 1.0, l.w, 1.0), 0.0, p.rule);
    }

    // The day's date, the notifications and Settings, and the world clocks.
    let date_r = R::new(l.date.x - 8.0, l.date.y + 10.0, l.date.w + 8.0, l.date.h - 20.0);
    button_bg(Hit::Today, date_r);
    pen.text(&locale::day_title(v.today), Font::Header, l.date, p.text, Align::Left);
    button_bg(Hit::Bell, l.bell);
    pen.glyph('\u{EA8F}', Font::Icon, l.bell, p.text);
    button_bg(Hit::Settings, l.gear);
    pen.glyph('\u{E713}', Font::Icon, l.gear, p.text);
    let here = time::day_of(Local.to_local(time::now()));
    for (r, clock) in l.clocks.iter().zip(&v.clocks) {
        if hovered(Hit::Clocks) {
            pen.fill(R::new(r.x - 8.0, r.y, r.w + 16.0, r.h), 4.0, p.hover);
        }
        let day = match time::day_of(clock.wall) - here {
            1 => " · tomorrow",
            -1 => " · yesterday",
            _ => "",
        };
        pen.text(&clock.name, Font::Caption, *r, p.text2, Align::Left);
        let right = format!("{}{day} · {}", super::zones::offset_words(clock.offset), locale::clock(time::second_of_day(clock.wall)));
        pen.text(&right, Font::Caption, *r, p.text2, Align::Right);
    }
    pen.fill(R::new(0.0, l.rule1, l.w, 1.0), 0.0, p.rule);

    if let Some(month) = l.month {
        draw_month(pen, v, month);
        pen.fill(R::new(0.0, l.rule2, l.w, 1.0), 0.0, p.rule);
    }

    // The agenda, or one event's details.
    if let Some(event) = v.detail.and_then(|i| v.agenda.get(i)) {
        button_bg(Hit::Back, l.back());
        pen.glyph('\u{E72B}', Font::IconSmall, l.back(), p.text);
        let t = l.agenda_title;
        pen.text(&event.title, Font::Strong, R::new(t.x + 28.0, t.y, l.w - t.x - 28.0 - layout::PAD, t.h), p.text, Align::Left);
        draw_detail(pen, v, event);
    } else {
        let title = match v.selected - v.today {
            0 => "Today".to_string(),
            1 => "Tomorrow".to_string(),
            -1 => "Yesterday".to_string(),
            _ => capitalised(&locale::day_title(v.selected)),
        };
        pen.text(&title, Font::Strong, l.agenda_title, p.text, Align::Left);
        if let (Some(prev), Some(next)) = (l.day_prev, l.day_next) {
            button_bg(Hit::DayPrev, prev);
            pen.glyph('\u{E76B}', Font::IconSmall, prev, p.text2);
            button_bg(Hit::DayNext, next);
            pen.glyph('\u{E76C}', Font::IconSmall, next, p.text2);
        }
        draw_agenda(pen, v);
    }

    draw_footer(pen, v);
}

fn draw_month(pen: &Pen, v: &View, month: R) {
    let p = &v.palette;
    let l = &v.layout;
    let hovered = |hit: Hit| v.hover == Some(hit);
    pen.text(&locale::month_title(v.month.0, v.month.1), Font::Strong, month, p.text, Align::Left);
    for (hit, r, glyph) in [(Hit::Prev, l.prev, '\u{E70E}'), (Hit::Next, l.next, '\u{E70D}')] {
        if hovered(hit) {
            pen.fill(r, 4.0, p.hover);
        }
        pen.glyph(glyph, Font::IconSmall, r, p.text2);
    }
    for (i, r) in l.weekdays.iter().enumerate() {
        let name = &v.weekdays[(v.first_weekday as usize + i) % 7];
        pen.text(name, Font::Body, *r, p.text, Align::Center);
    }
    let start = v.grid_start();
    for (row, r) in l.weeks.iter().enumerate() {
        let week = layout::iso_week(start + row as i32 * 7 + 3);
        pen.text(&week.to_string(), Font::Caption, *r, p.accent, Align::Left);
    }
    let dots = model::dots(&v.events, start, 42, &Local);
    for (i, cell) in l.cells.iter().enumerate() {
        let day = start + i as i32;
        let (_, m, d) = time::civil_from_days(day);
        let center = (cell.x + cell.w / 2.0, cell.y + cell.h / 2.0 - 1.0);
        let radius = (cell.h / 2.0 - 3.0).min(cell.w / 2.0 - 2.0);
        let today = day == v.today;
        if today {
            pen.circle(center, radius, p.accent);
        } else if hovered(Hit::Day(i)) {
            pen.circle(center, radius, p.hover);
        }
        if day == v.selected && !today {
            pen.ring(center, radius, 1.5, p.accent);
        }
        let color = if today { p.on_accent } else if m != v.month.1 { p.text3 } else { p.text };
        pen.text(&d.to_string(), Font::Body, R::new(cell.x, center.1 - 10.0, cell.w, 20.0), color, Align::Center);
        let marks = &dots[i];
        let n = marks.len() as f32;
        for (k, rgb) in marks.iter().enumerate() {
            let x = center.0 - (n - 1.0) * 3.0 + k as f32 * 6.0;
            let c = if today { p.on_accent } else { Color::rgb(*rgb) };
            pen.circle((x, center.1 + radius - 5.0), 1.75, c);
        }
    }
}

fn draw_agenda(pen: &Pen, v: &View) {
    let p = &v.palette;
    let l = &v.layout;
    if v.agenda.is_empty() {
        let (text, second) = if !v.has_sources {
            ("No calendar connected", Some("Connect Google, Outlook or a calendar link in Settings"))
        } else if super::fetching() && v.events.is_empty() {
            ("Loading…", None)
        } else {
            ("No events", None)
        };
        let r = l.agenda;
        pen.text(text, Font::Body, R::new(r.x + layout::PAD, r.y + 4.0, r.w - 2.0 * layout::PAD, 24.0), p.text2, Align::Left);
        if let Some(second) = second {
            pen.text(second, Font::Caption, R::new(r.x + layout::PAD, r.y + 28.0, r.w - 2.0 * layout::PAD, 18.0), p.text3, Align::Left);
        }
        return;
    }
    pen.clip(l.agenda);
    for (i, event) in v.agenda.iter().enumerate() {
        let r = l.row(i, v.scroll);
        if r.bottom() < l.agenda.y || r.y > l.agenda.bottom() {
            continue;
        }
        if v.hover == Some(Hit::Event(i)) {
            pen.fill(r, 4.0, p.hover);
        }
        let time_r = R::new(r.x + 8.0, r.y, 60.0, r.h);
        match event.when {
            When::AllDay { .. } => pen.text("All day", Font::Caption, time_r, p.text2, Align::Left),
            When::At { start, end } => {
                let (first, last) = times(v.selected, start, end);
                pen.text(&first, Font::Body, R::new(time_r.x, r.y + 8.0, time_r.w, 18.0), p.text, Align::Left);
                if end > start {
                    pen.text(&last, Font::Caption, R::new(time_r.x, r.y + 28.0, time_r.w, 16.0), p.text2, Align::Left);
                }
            }
        }
        pen.fill(R::new(r.x + 72.0, r.y + 9.0, 3.0, r.h - 18.0), 1.5, Color::rgb(event.color));
        let join = event.join.as_ref().map(|_| l.join(r));
        let text_w = join.map_or(r.right() - 8.0, |j| j.x - 8.0) - (r.x + 84.0);
        pen.text(&event.title, Font::Body, R::new(r.x + 84.0, r.y + 8.0, text_w, 18.0), p.text, Align::Left);
        let second = if event.location.is_empty() { source_name(v, event) } else { event.location.clone() };
        pen.text(&second, Font::Caption, R::new(r.x + 84.0, r.y + 28.0, text_w, 16.0), p.text2, Align::Left);
        if let Some(j) = join {
            let fill = if v.hover == Some(Hit::Join(i)) { p.accent.alpha(0.85) } else { p.accent };
            pen.fill(j, 4.0, fill);
            pen.text("Join", Font::CaptionStrong, j, p.on_accent, Align::Center);
        }
    }
    pen.unclip();
}

/// The start and end shown for a timed event on `day` ("…" where it runs over from another day).
fn times(day: i32, start: i64, end: i64) -> (String, String) {
    let (s, e) = (Local.to_local(start), Local.to_local(end));
    let first = if time::day_of(s) == day { locale::clock(time::second_of_day(s)) } else { "…".into() };
    let last = if time::day_of(e - 1) == day || end == start { locale::clock(time::second_of_day(e)) } else { "…".into() };
    (first, last)
}

fn draw_detail(pen: &Pen, v: &View, event: &Event) {
    let p = &v.palette;
    let l = &v.layout;
    let area = l.agenda;
    let x = layout::PAD;
    let w = area.w - 2.0 * layout::PAD;
    let mut y = area.y + 2.0;
    let mut line = |text: &str, glyph: char, color: Color| {
        if text.is_empty() {
            return;
        }
        pen.glyph(glyph, Font::IconSmall, R::new(x, y, 16.0, 22.0), p.text2);
        pen.text(text, Font::Body, R::new(x + 24.0, y, w - 24.0, 22.0), color, Align::Left);
        y += 24.0;
    };
    let when = match event.when {
        When::AllDay { first, end } if end - first > 1 => format!("{} – {}", locale::day_title(first), locale::day_title(end - 1)),
        When::AllDay { .. } => "All day".to_string(),
        When::At { start, end } => {
            let (a, b) = times(v.selected, start, end);
            format!("{a} – {b}")
        }
    };
    line(&when, '\u{E823}', p.text);
    line(&event.location, '\u{E707}', p.text);
    let d = &event.details;
    let who = match (d.organizer.is_empty(), d.response.is_empty()) {
        (false, false) => format!("{} · you: {}", d.organizer, d.response.to_lowercase()),
        (false, true) => d.organizer.clone(),
        (true, false) => format!("You: {}", d.response.to_lowercase()),
        (true, true) => String::new(),
    };
    line(&who, '\u{E77B}', p.text2);
    if !d.people.is_empty() {
        let shown: Vec<&str> = d.people.iter().take(4).map(String::as_str).collect();
        let more = d.people.len().saturating_sub(4);
        let people = if more > 0 { format!("{} +{more}", shown.join(", ")) } else { shown.join(", ") };
        line(&people, '\u{E716}', p.text2);
    }
    line(&source_name(v, event), '\u{E787}', p.text3);
    let (join_r, open_r) = l.detail_buttons();
    if !d.notes.is_empty() {
        let notes = R::new(x, y + 4.0, w, (join_r.y - 8.0 - (y + 4.0)).max(0.0));
        pen.clip(notes);
        pen.text(&d.notes, Font::Paragraph, notes, p.text2, Align::Left);
        pen.unclip();
    }
    let mut buttons = Vec::new();
    if event.join.is_some() {
        buttons.push((Hit::DetailJoin, "Join", true));
    }
    if event.link.is_some() {
        buttons.push((Hit::DetailOpen, "Open in calendar", false));
    }
    for ((hit, label, accent), r) in buttons.into_iter().zip([join_r, open_r]) {
        let fill = match (accent, v.hover == Some(hit)) {
            (true, false) => p.accent,
            (true, true) => p.accent.alpha(0.85),
            (false, false) => p.raised,
            (false, true) => p.hover,
        };
        pen.fill(r, 4.0, fill);
        pen.text(label, Font::CaptionStrong, r, if accent { p.on_accent } else { p.text }, Align::Center);
    }
}

fn draw_footer(pen: &Pen, v: &View) {
    let p = &v.palette;
    let l = &v.layout;
    let hovered = |hit: Hit| v.hover == Some(hit);
    pen.fill(R::new(0.0, l.footer.y, l.w, 1.0), 0.0, p.rule);
    let running = super::schedule::focus_end();
    let left = R::new(l.footer.x, l.footer.y, l.focus_less.x - l.footer.x - 8.0, l.footer.h);
    match running {
        Some(end) => {
            let minutes = ((end - time::now()) + 59) / 60;
            pen.text(&format!("Focus · {minutes} min left"), Font::Caption, left, p.text, Align::Left);
        }
        None => {
            let (status, warn) = status_line(v);
            pen.text(&status, Font::Caption, left, if warn { p.warn } else { p.text2 }, Align::Left);
            for (hit, r, glyph) in [(Hit::FocusLess, l.focus_less, '\u{E738}'), (Hit::FocusMore, l.focus_more, '\u{E710}')] {
                if hovered(hit) {
                    pen.fill(r, 4.0, p.hover);
                }
                pen.glyph(glyph, Font::IconSmall, r, p.text2);
            }
            let minutes = R::new(l.focus_less.right(), l.focus_less.y, l.focus_more.x - l.focus_less.right(), l.focus_less.h);
            pen.text(&format!("{} min", v.focus_minutes), Font::CaptionStrong, minutes, p.text, Align::Center);
        }
    }
    let b = l.focus_button;
    pen.fill(b, 4.0, if hovered(Hit::Focus) { p.hover } else { p.raised });
    pen.stroke(b, 4.0, 1.0, p.rule);
    let (glyph, word) = if running.is_some() { ('\u{E71A}', "Stop") } else { ('\u{E768}', "Focus") };
    pen.glyph(glyph, Font::IconSmall, R::new(b.x + 10.0, b.y, 16.0, b.h), p.text);
    pen.text(word, Font::CaptionStrong, R::new(b.x + 26.0, b.y, b.w - 32.0, b.h), p.text, Align::Center);
}

/// Which calendar an event is from.
fn source_name(v: &View, event: &Event) -> String {
    let status = super::status(&v.setup);
    if let Some(calendar) = status.calendars.iter().find(|c| c.key == event.calendar) {
        return calendar.name.clone();
    }
    match event.source {
        super::Source::Google => "Google".into(),
        super::Source::Microsoft => "Outlook".into(),
        super::Source::Link(i) => v.setup.links.get(i).map_or_else(|| "Calendar".into(), |l| l.name.clone()),
    }
}

/// The footer: the first problem, else whether the events are being updated or when they were.
fn status_line(v: &View) -> (String, bool) {
    if !v.has_sources {
        return (String::new(), false);
    }
    let status = super::status(&v.setup);
    for provider in super::Provider::ALL {
        if let Some(problem) = &status.account(provider).problem {
            return (format!("{}: {problem}", if provider == super::Provider::Microsoft { "Outlook" } else { "Google" }), true);
        }
    }
    if let Some((i, problem)) = status.links.iter().enumerate().find_map(|(i, p)| p.as_ref().map(|p| (i, p))) {
        let name = v.setup.links.get(i).map_or("A link", |l| l.name.as_str());
        return (format!("{name}: {problem}"), true);
    }
    if super::fetching() {
        return ("Updating…".into(), false);
    }
    match status.fetched {
        Some(at) => (format!("Updated {}", locale::clock(time::second_of_day(Local.to_local(at)))), false),
        None => (String::new(), false),
    }
}

fn capitalised(text: &str) -> String {
    let mut chars = text.chars();
    chars.next().map_or_else(String::new, |c| c.to_uppercase().chain(chars).collect())
}

/// Windows' notification centre (Win+N).
fn open_notifications() {
    let key = |vk: VIRTUAL_KEY, up: bool| INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 { ki: KEYBDINPUT { wVk: vk, dwFlags: if up { KEYEVENTF_KEYUP } else { Default::default() }, ..Default::default() } },
    };
    let n = VIRTUAL_KEY(b'N' as u16);
    let inputs = [key(VK_LWIN, false), key(n, false), key(n, true), key(VK_LWIN, true)];
    unsafe { SendInput(&inputs, std::mem::size_of::<INPUT>() as i32) };
}

/// The arrow cursor, whatever was showing when the capture began.
fn arrow() {
    use windows::Win32::UI::WindowsAndMessaging::{LoadCursorW, SetCursor, IDC_ARROW};
    unsafe { SetCursor(LoadCursorW(None, IDC_ARROW).ok()) };
}

fn client_xy(lparam: LPARAM) -> (i32, i32) {
    ((lparam.0 & 0xFFFF) as u16 as i16 as i32, ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32)
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // A panic must not unwind into user32.
    let handled = std::panic::catch_unwind(|| handle(hwnd, msg, wparam, lparam)).unwrap_or_else(|_| {
        eprintln!("calendar: a message handler panicked");
        Some(LRESULT(0))
    });
    handled.unwrap_or_else(|| unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) })
}

fn handle(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> Option<LRESULT> {
    let mut after: Option<After> = None;
    match msg {
        WM_MOUSEMOVE => {
            arrow();
            let (x, y) = client_xy(lparam);
            with_view(|v| {
                let hit = v.hit(x, y);
                v.set_hover(hit);
            });
        }
        WM_LBUTTONDOWN | WM_RBUTTONDOWN => {
            let (x, y) = client_xy(lparam);
            let inside = with_view(|v| {
                let inside = x >= 0 && y >= 0 && x < v.size.0 && y < v.size.1;
                v.pressed = v.hit(x, y);
                v.render();
                inside
            });
            if inside == Some(false) {
                // Under capture: a click outside the calendar closes it.
                after = Some(Box::new(close));
            }
        }
        WM_LBUTTONUP => {
            let (x, y) = client_xy(lparam);
            after = with_view(|v| {
                let pressed = v.pressed.take();
                let hit = v.hit(x, y);
                let done = pressed.filter(|p| Some(*p) == hit).and_then(|hit| v.activate(hit));
                if pressed.is_some() {
                    v.render();
                }
                done
            })
            .flatten();
        }
        WM_MOUSEWHEEL => {
            let delta = (wparam.0 >> 16) as u16 as i16 as i32;
            // The wheel's point is on the screen.
            let mut point = POINT { x: (lparam.0 & 0xFFFF) as u16 as i16 as i32, y: ((lparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32 };
            let _ = unsafe { ScreenToClient(hwnd, &mut point) };
            with_view(|v| v.wheel(delta, point.x, point.y));
        }
        WM_KEYDOWN => {
            after = with_view(|v| v.key(VIRTUAL_KEY(wparam.0 as u16))).flatten();
        }
        WM_ACTIVATE if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE => {
            after = Some(Box::new(close));
        }
        WM_CAPTURECHANGED => {
            // Capture taken away (another app came up): give way to it. Our own release in
            // WM_DESTROY arrives while closing, when there is nothing left to close.
            if OPEN.with(Cell::get) == hwnd.0 as isize && unsafe { GetCapture() } != hwnd {
                after = Some(Box::new(close));
            }
        }
        WM_DESTROY => {
            OPEN.with(|o| o.set(0));
            *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner()) = None;
            CLOSED_AT.with(|c| c.set(Some(Instant::now())));
            drop(VIEW.with(|cell| cell.borrow_mut().take()));
            let _ = unsafe { ReleaseCapture() };
        }
        _ => return None,
    }
    if let Some(work) = after {
        work();
    }
    Some(LRESULT(0))
}

/// Dev: the calendar alone, on the config's sources, until it closes.
#[cfg(feature = "dev")]
pub fn preview(config: &Config) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, SetTimer, TranslateMessage, MSG};
    super::notifications::start();
    if let Err(e) = create(config) {
        eprintln!("calendar: {e:#}");
        return;
    }
    // A thread timer, so a finished fetch shows without waiting for the pointer to move.
    unsafe { SetTimer(None, 0, 250, None) };
    let mut msg = MSG::default();
    while OPEN.with(Cell::get) != 0 && unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        refresh_if_done();
    }
}

#[cfg(feature = "dev")]
fn refresh_if_done() {
    thread_local! { static WAS_FETCHING: Cell<bool> = const { Cell::new(false) }; }
    let now = super::fetching();
    if WAS_FETCHING.with(|w| w.replace(now)) && !now {
        refresh();
    }
}
