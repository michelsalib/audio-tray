//! The calendar flyout: the day's date, a month grid with a dot per day that has events, and the
//! selected day's agenda. It opens over the clock's corner like the shell's own and closes on a
//! click elsewhere, Escape, or losing the foreground.
//!
//! **Re-entrancy.** Opening a link or Settings can pump messages: that work runs after the view's
//! state is released ([`After`]), as in the settings window.

mod layout;
mod locale;
mod paint;

use std::cell::{Cell, RefCell};
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use windows::core::{w, BOOL, PCWSTR};
use windows::Win32::Foundation::{COLORREF, HWND, LPARAM, LRESULT, POINT, WPARAM};
use windows::Win32::Graphics::Dwm::{
    DwmSetWindowAttribute, DWMWA_USE_IMMERSIVE_DARK_MODE, DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_ROUND,
};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, ScreenToClient, UpdateWindow, PAINTSTRUCT};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetCapture, ReleaseCapture, SendInput, SetCapture, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYEVENTF_KEYUP, VIRTUAL_KEY,
    VK_DOWN, VK_ESCAPE, VK_HOME, VK_LEFT, VK_LWIN, VK_NEXT, VK_PRIOR, VK_RIGHT, VK_UP,
};
use windows::Win32::UI::WindowsAndMessaging::{
    DefWindowProcW, DestroyWindow, GetCursorPos, ShowWindow, SW_SHOW, WA_INACTIVE, WHEEL_DELTA,
    WINDOW_EX_STYLE, WM_ACTIVATE, WM_CAPTURECHANGED, WM_DESTROY, WM_ERASEBKGND, WM_KEYDOWN, WM_LBUTTONDOWN, WM_LBUTTONUP,
    WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_PAINT, WM_RBUTTONDOWN, WS_EX_LAYERED, WS_EX_TOOLWINDOW, WS_EX_TOPMOST,
    SetLayeredWindowAttributes, LWA_ALPHA,
};

use super::model::{self, Event, When};
use super::time::{self, Local, Zone};
use super::Setup;
use layout::{Hit, Layout, R};
use paint::{Align, Color, Font, Gfx, Palette, Pen};

const CLASS: PCWSTR = w!("AudioTrayCalendar");
/// A click on the clock right after the calendar closed is the click that closed it.
const SETTLE: Duration = Duration::from_millis(400);
/// The gap between the calendar and the screen's edges, as the shell's flyouts keep.
const MARGIN: f32 = 12.0;
/// How far a wheel notch scrolls the agenda.
const SCROLL_STEP: f32 = 48.0;

thread_local! {
    static VIEW: RefCell<Option<View>> = const { RefCell::new(None) };
    static OPEN: Cell<isize> = const { Cell::new(0) };
    static CLOSED_AT: Cell<Option<Instant>> = const { Cell::new(None) };
    /// The close animation is running.
    static CLOSING: Cell<bool> = const { Cell::new(false) };
    /// How far it slides, in pixels.
    static SLIDE_PX: Cell<f32> = const { Cell::new(0.0) };
}

struct View {
    hwnd: HWND,
    gfx: RefCell<Gfx>,
    palette: Palette,
    scale: f32,
    size: (u32, u32),
    layout: Layout,
    setup: Setup,
    has_sources: bool,
    today: i32,
    /// The month shown.
    month: (i32, u32),
    selected: i32,
    first_weekday: u32,
    /// Short day names, Monday first.
    weekdays: [String; 7],
    events: Vec<Event>,
    /// The selected day's events.
    agenda: Vec<Event>,
    scroll: f32,
    hover: Option<Hit>,
    pressed: Option<Hit>,
}

/// Work to do once the view's state is released: it may pump messages.
type After = Box<dyn FnOnce()>;

/// Open the calendar, or close it if it is open.
pub fn toggle(config: &crate::config::Config) {
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
    let rest = *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(r) = rest {
        crate::win::animate_window(hwnd, crate::win::Flyout::Close, (r.left, r.top), SLIDE_PX.with(Cell::get));
    }
    let _ = unsafe { DestroyWindow(hwnd) };
    CLOSING.with(|c| c.set(false));
}

/// The events changed: show them.
pub fn refresh() {
    with_view(|v| {
        v.reload();
        v.invalidate();
    });
}

fn with_view<R>(f: impl FnOnce(&mut View) -> R) -> Option<R> {
    VIEW.with(|cell| cell.try_borrow_mut().ok().and_then(|mut v| v.as_mut().map(f)))
}

fn create(config: &crate::config::Config) -> Result<()> {
    let mut cursor = POINT::default();
    let _ = unsafe { GetCursorPos(&mut cursor) };
    let (scale, work) = crate::win::monitor_at(cursor);
    let room = (work.bottom - work.top) as f32 / scale - 2.0 * MARGIN;
    let layout = layout::layout(layout::rows_for(room));
    let (w, h) = ((layout.w * scale).round() as i32, (layout.h * scale).round() as i32);
    let margin = (MARGIN * scale).round() as i32;
    // Over the clock's corner: the work area's bottom right, wherever the taskbar is.
    let (x, y) = (work.right - w - margin, work.bottom - h - margin);
    let hwnd = crate::win::create_popup(CLASS, w!("Calendar"), Some(wndproc), WINDOW_EX_STYLE(WS_EX_TOOLWINDOW.0 | WS_EX_TOPMOST.0 | WS_EX_LAYERED.0), (x, y, w, h))
        .context("CreateWindowExW")?;
    let palette = Palette::current();
    unsafe {
        let dark = BOOL::from(palette.dark());
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, &dark as *const _ as _, std::mem::size_of::<BOOL>() as u32);
        let round = DWMWCP_ROUND;
        let _ = DwmSetWindowAttribute(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, &round as *const _ as _, std::mem::size_of_val(&round) as u32);
    }
    let today = time::today();
    let (ty, tm, _) = time::civil_from_days(today);
    let setup = config.calendar.setup();
    let mut view = View {
        hwnd,
        gfx: RefCell::new(Gfx::new()?),
        palette,
        scale,
        size: (w as u32, h as u32),
        layout,
        has_sources: super::has_sources(&setup),
        setup,
        today,
        month: (ty, tm),
        selected: today,
        first_weekday: locale::first_weekday(),
        weekdays: locale::weekday_names(),
        events: Vec::new(),
        agenda: Vec::new(),
        scroll: 0.0,
        hover: None,
        pressed: None,
    };
    view.reload();
    view.fetch();
    VIEW.with(|cell| *cell.borrow_mut() = Some(view));
    OPEN.with(|o| o.set(hwnd.0 as isize));
    *super::OPEN_RECT.lock().unwrap_or_else(|e| e.into_inner()) = Some(windows::Win32::Foundation::RECT { left: x, top: y, right: x + w, bottom: y + h });
    unsafe {
        // Shown transparent, painted, then brought in.
        let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 0, LWA_ALPHA);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = UpdateWindow(hwnd);
        // The click that opened it was swallowed by our hook, so Windows has not lent us the
        // foreground: without it there are no keys (Escape) and no deactivation to close on.
        let raised = crate::music::player::raise(hwnd);
        if raised == crate::music::player::Raise::Refused {
            eprintln!("calendar: could not take the foreground");
        }
        let slide = crate::win::FLYOUT_SLIDE * scale;
        SLIDE_PX.with(|s| s.set(slide));
        crate::win::animate_window(hwnd, crate::win::Flyout::Open, (x, y), slide);
        // Like a menu: a click anywhere else comes here first, and closes it.
        SetCapture(hwnd);
        // No WM_SETCURSOR arrives under capture: the arrow is set here, and on every move.
        arrow();
    }
    Ok(())
}

impl View {
    fn invalidate(&self) {
        let _ = unsafe { InvalidateRect(Some(self.hwnd), None, false) };
    }

    /// Take the cache's events, and the selected day's. The month shown stays where it is.
    fn reload(&mut self) {
        self.events = super::events().0;
        self.agenda = model::on_day(&self.events, self.selected, &Local).into_iter().cloned().collect();
    }

    fn fetch(&self) {
        if self.has_sources {
            super::fetch(&self.setup, super::days_around(self.month.0, self.month.1), false);
        }
    }

    fn select(&mut self, day: i32) {
        if day != self.selected {
            self.scroll = 0.0;
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
        layout::hit(&self.layout, x, y, self.scroll, self.agenda.len(), &|i| self.agenda.get(i).is_some_and(|e| e.join.is_some()))
    }

    fn set_hover(&mut self, hover: Option<Hit>) {
        if self.hover != hover {
            self.hover = hover;
            self.invalidate();
        }
    }

    /// Act on a click; what needs the state released comes back.
    fn activate(&mut self, hit: Hit) -> Option<After> {
        match hit {
            Hit::Today => {
                self.today = time::today();
                self.select(self.today);
            }
            Hit::Prev => self.show_month(-1),
            Hit::Next => self.show_month(1),
            Hit::Day(i) => self.select(self.grid_start() + i as i32),
            Hit::Bell => return Some(Box::new(|| {
                close();
                open_notifications();
            })),
            Hit::Settings => return Some(Box::new(|| {
                close();
                crate::tray::post(crate::settings::WM_OPEN_SETTINGS, crate::settings::OPEN_CALENDAR, 0);
            })),
            Hit::Event(i) | Hit::Join(i) => {
                let event = self.agenda.get(i)?;
                let url = if hit == Hit::Join(i) { event.join.clone() } else { event.link.clone().or_else(|| event.join.clone()) };
                let url = url?;
                return Some(Box::new(move || {
                    close();
                    crate::win::shell_open(&url, None);
                }));
            }
        }
        self.invalidate();
        None
    }

    fn key(&mut self, key: VIRTUAL_KEY) -> Option<After> {
        let day = match key {
            VK_ESCAPE => return Some(Box::new(close)),
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
        self.invalidate();
        None
    }

    fn wheel(&mut self, delta: i32, x: i32, y: i32) {
        let notches = delta as f32 / WHEEL_DELTA as f32;
        let (x, y) = (x as f32 / self.scale, y as f32 / self.scale);
        if self.layout.agenda.contains(x, y) {
            let max = self.layout.max_scroll(self.agenda.len());
            self.scroll = (self.scroll - notches * SCROLL_STEP).clamp(0.0, max);
        } else if delta != 0 {
            // Over the month, as in the shell's calendar: a notch is a month.
            self.show_month(if delta > 0 { -1 } else { 1 });
        }
        self.invalidate();
    }

    fn paint(&self) {
        let result = self.gfx.borrow_mut().frame(self.hwnd, self.size, self.scale, |pen| draw(pen, self));
        if let Err(e) = result {
            eprintln!("calendar: could not paint ({e:#})");
        }
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

    // The day's date, the notifications and Settings.
    let date_text = locale::day_title(v.today);
    let date_r = R::new(l.date.x - 8.0, l.date.y + 10.0, l.date.w + 8.0, l.date.h - 20.0);
    button_bg(Hit::Today, date_r);
    pen.text(&date_text, Font::Header, R::new(l.date.x, l.date.y, l.date.w, l.date.h), p.text, Align::Left);
    button_bg(Hit::Bell, l.bell);
    pen.glyph('\u{EA8F}', Font::Icon, l.bell, p.text);
    button_bg(Hit::Settings, l.gear);
    pen.glyph('\u{E713}', Font::Icon, l.gear, p.text);
    pen.fill(R::new(0.0, l.rule1, l.w, 1.0), 0.0, p.rule);

    // The month, and its arrows.
    pen.text(&locale::month_title(v.month.0, v.month.1), Font::Strong, l.month, p.text, Align::Left);
    button_bg(Hit::Prev, l.prev);
    pen.glyph('\u{E70E}', Font::IconSmall, l.prev, p.text2);
    button_bg(Hit::Next, l.next);
    pen.glyph('\u{E70D}', Font::IconSmall, l.next, p.text2);
    for (i, r) in l.weekdays.iter().enumerate() {
        let name = &v.weekdays[(v.first_weekday as usize + i) % 7];
        pen.text(name, Font::Body, *r, p.text, Align::Center);
    }

    // The days.
    let start = v.grid_start();
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
    pen.fill(R::new(0.0, l.rule2, l.w, 1.0), 0.0, p.rule);

    // The selected day's agenda.
    let title = match v.selected - v.today {
        0 => "Today".to_string(),
        1 => "Tomorrow".to_string(),
        -1 => "Yesterday".to_string(),
        _ => capitalised(&locale::day_title(v.selected)),
    };
    pen.text(&title, Font::Strong, l.agenda_title, p.text, Align::Left);
    draw_agenda(pen, v);

    // What is going on with the sources.
    let (status, warn) = status_line(v);
    pen.text(&status, Font::Caption, l.footer, if warn { p.warn } else { p.text2 }, Align::Left);
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
        // When: start and end, or all day.
        let time_r = R::new(r.x + 8.0, r.y, 60.0, r.h);
        match event.when {
            When::AllDay { .. } => pen.text("All day", Font::Caption, time_r, p.text2, Align::Left),
            When::At { start, end } => {
                let (start_wall, end_wall) = (Local.to_local(start), Local.to_local(end));
                let first = if time::day_of(start_wall) == v.selected { locale::clock(time::second_of_day(start_wall)) } else { "…".into() };
                let last = if time::day_of(end_wall - 1) == v.selected || end == start { locale::clock(time::second_of_day(end_wall)) } else { "…".into() };
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

/// Which account or link an event is from.
fn source_name(v: &View, event: &Event) -> String {
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
        WM_PAINT => {
            let mut ps = PAINTSTRUCT::default();
            unsafe { BeginPaint(hwnd, &mut ps) };
            with_view(|v| v.paint());
            let _ = unsafe { EndPaint(hwnd, &ps) };
        }
        WM_ERASEBKGND => return Some(LRESULT(1)),
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
                let inside = x >= 0 && y >= 0 && (x as u32) < v.size.0 && (y as u32) < v.size.1;
                v.pressed = v.hit(x, y);
                v.invalidate();
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
                v.invalidate();
                pressed.filter(|p| Some(*p) == hit).and_then(|hit| v.activate(hit))
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
pub fn preview(config: &crate::config::Config) {
    use windows::Win32::UI::WindowsAndMessaging::{DispatchMessageW, GetMessageW, TranslateMessage, MSG};
    if let Err(e) = create(config) {
        eprintln!("calendar: {e:#}");
        return;
    }
    // A thread timer, so a finished fetch shows without waiting for the pointer to move.
    unsafe { windows::Win32::UI::WindowsAndMessaging::SetTimer(None, 0, 250, None) };
    let mut msg = MSG::default();
    while OPEN.with(Cell::get) != 0 && unsafe { GetMessageW(&mut msg, None, 0, 0) }.as_bool() {
        unsafe {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        // No tray to post to: the preview looks for finished fetches itself.
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
